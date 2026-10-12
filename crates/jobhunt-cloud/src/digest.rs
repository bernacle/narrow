//! When the daily digest goes out (BRU-389 slice A).
//!
//! The rule is about the person's calendar, not a clock interval: **at
//! most one digest per local calendar day**, composed at their digest time
//! on the days they chose, never inside their quiet hours or while paused.
//!
//! * The slot of a day is the digest time on that local date. On a
//!   daylight-saving change it is the earlier of two repeated times, or the
//!   first valid time after a skipped one (02:30 on a spring-forward night
//!   becomes 03:00).
//! * A slot is used at most once ([`crate::notify`] claims the local date
//!   in the database, whether the digest finds anything or not), so neither
//!   a 23- or 25-hour day nor two workers can make a second one.
//! * A slot is due from its time until [`GRACE_MINUTES`] later. A worker
//!   that did not run in that window does not send it later: the next
//!   digest is the next slot. Missed digests are never caught up.
//! * Inside quiet hours nothing is composed (a digest time inside them is
//!   refused when it is set, see [`Schedule::validate`]), and a composed
//!   digest the provider has not accepted by the start of quiet hours, the
//!   end of the day or [`DELIVERY_WINDOW_HOURS`] is abandoned
//!   ([`Schedule::deliver_by`]).

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use jobhunt_storage::postgres::DigestSettings;

/// How long after its time a digest may still be composed (the worker runs
/// every 15 minutes).
pub const GRACE_MINUTES: i64 = 60;
/// How long after its time a composed digest may still be delivered
/// (provider retries).
pub const DELIVERY_WINDOW_HOURS: i64 = 6;

/// Day names, Monday first (bit 0), as the API spells them.
pub const DAY_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A person's digest schedule, resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub tz: Tz,
    pub time: NaiveTime,
    /// Bitmask from Monday (1) to Sunday (64).
    pub days: u8,
    /// Local start and end (may wrap past midnight); `None`: no quiet
    /// hours.
    pub quiet: Option<(NaiveTime, NaiveTime)>,
    pub paused_until: Option<DateTime<Utc>>,
}

/// Whether a digest is due now, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Compose the digest of local date `day`, whose time was `at`.
    Due {
        day: NaiveDate,
        at: DateTime<Utc>,
    },
    Paused,
    /// Today's digest slot was already used.
    AlreadyUsed,
    /// Not a digest day.
    DayOff,
    /// Later today.
    NotYet,
    /// Today's time passed more than [`GRACE_MINUTES`] ago.
    Missed,
    /// Inside quiet hours.
    Quiet,
}

/// The timezone of `settings`: theirs, or UTC when unset or unknown.
pub fn timezone(settings: &DigestSettings) -> Tz {
    settings
        .timezone
        .as_deref()
        .and_then(|name| name.parse::<Tz>().ok())
        .unwrap_or(Tz::UTC)
}

/// The instant a local wall-clock time happens: the earlier one when it
/// happens twice (fall back), the first valid one after it when it is
/// skipped (spring forward).
pub fn local_instant(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    for minutes in 0..=24 * 60 {
        if let Some(at) = tz
            .from_local_datetime(&(local + Duration::minutes(minutes)))
            .earliest()
        {
            return at.with_timezone(&Utc);
        }
    }
    // No timezone skips a whole day; read it as UTC if one ever does.
    local.and_utc()
}

/// Whether `t` is inside the quiet hours `start`–`end` (wrapping past
/// midnight when `start` is later than `end`; none when they are equal).
pub fn in_quiet_hours(t: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
    match start.cmp(&end) {
        std::cmp::Ordering::Less => start <= t && t < end,
        std::cmp::Ordering::Greater => t >= start || t < end,
        std::cmp::Ordering::Equal => false,
    }
}

impl Schedule {
    pub fn of(settings: &DigestSettings) -> Self {
        Self {
            tz: timezone(settings),
            time: settings.time,
            days: settings.days,
            quiet: (settings.quiet_start != settings.quiet_end)
                .then_some((settings.quiet_start, settings.quiet_end)),
            paused_until: settings.paused_until,
        }
    }

    /// Why these settings can't be used, if they can't: a digest time
    /// inside quiet hours (it would never be sent), or no day at all.
    pub fn validate(settings: &DigestSettings) -> Result<(), String> {
        if settings.days & 0x7f == 0 {
            return Err("choose at least one day for the digest".into());
        }
        if !(1..=5).contains(&settings.size) {
            return Err("a digest holds 1 to 5 jobs".into());
        }
        if in_quiet_hours(settings.time, settings.quiet_start, settings.quiet_end) {
            return Err(format!(
                "the digest time {} is inside your quiet hours ({}–{}); choose a time outside them",
                settings.time.format("%H:%M"),
                settings.quiet_start.format("%H:%M"),
                settings.quiet_end.format("%H:%M"),
            ));
        }
        Ok(())
    }

    fn allows(&self, day: NaiveDate) -> bool {
        self.days & (1 << day.weekday().num_days_from_monday()) != 0
    }

    /// The instant of the digest time on local date `day`.
    pub fn slot_on(&self, day: NaiveDate) -> DateTime<Utc> {
        local_instant(self.tz, day.and_time(self.time))
    }

    fn quiet_at(&self, at: DateTime<Utc>) -> bool {
        self.quiet.is_some_and(|(start, end)| {
            in_quiet_hours(at.with_timezone(&self.tz).time(), start, end)
        })
    }

    /// Whether a digest is due at `now`, given the local date of the last
    /// slot used.
    pub fn check(&self, last_used: Option<NaiveDate>, now: DateTime<Utc>) -> Slot {
        if self.paused_until.is_some_and(|until| now < until) {
            return Slot::Paused;
        }
        let today = now.with_timezone(&self.tz).date_naive();
        if last_used.is_some_and(|last| last >= today) {
            return Slot::AlreadyUsed;
        }
        if !self.allows(today) {
            return Slot::DayOff;
        }
        let at = self.slot_on(today);
        if now < at {
            return Slot::NotYet;
        }
        if now - at >= Duration::minutes(GRACE_MINUTES) {
            return Slot::Missed;
        }
        if self.quiet_at(now) {
            return Slot::Quiet;
        }
        Slot::Due { day: today, at }
    }

    /// The next slot at or after `now` that could be used (for the
    /// settings page), within two weeks.
    pub fn next(&self, last_used: Option<NaiveDate>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let from = self.paused_until.map_or(now, |p| p.max(now));
        let today = from.with_timezone(&self.tz).date_naive();
        (0..15)
            .filter_map(|d| today.checked_add_days(chrono::Days::new(d)))
            .filter(|day| self.allows(*day) && last_used.is_none_or(|last| last < *day))
            .map(|day| self.slot_on(day))
            .find(|at| *at + Duration::minutes(GRACE_MINUTES) > from && !self.quiet_at(*at))
            .map(|at| at.max(from))
    }

    /// When a digest composed for the slot at `at` (local date `day`) must
    /// have been accepted by the provider: within
    /// [`DELIVERY_WINDOW_HOURS`], before quiet hours start, and on the
    /// same local day.
    pub fn deliver_by(&self, day: NaiveDate, at: DateTime<Utc>) -> DateTime<Utc> {
        let mut by = at + Duration::hours(DELIVERY_WINDOW_HOURS);
        if let Some(next) = day.succ_opt() {
            by = by.min(local_instant(self.tz, next.and_time(NaiveTime::MIN)));
        }
        if let Some((start, _)) = self.quiet
            && start > self.time
        {
            by = by.min(local_instant(self.tz, day.and_time(start)));
        }
        by
    }
}

/// The days of a bitmask, as the API spells them.
pub fn day_names(days: u8) -> Vec<String> {
    DAY_NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| days & (1 << i) != 0)
        .map(|(_, d)| (*d).to_owned())
        .collect()
}

/// The bitmask of day names (`mon` … `sun`), or the first unknown name.
pub fn days_of(names: &[String]) -> Result<u8, String> {
    let mut days = 0u8;
    for name in names {
        let i = DAY_NAMES
            .iter()
            .position(|d| d.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| name.clone())?;
        days |= 1 << i;
    }
    Ok(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobhunt_storage::postgres::WEEKDAYS;
    use std::collections::BTreeMap;

    fn hm(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn settings(tz: &str) -> DigestSettings {
        DigestSettings {
            timezone: Some(tz.into()),
            ..DigestSettings::default()
        }
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Runs the worker every 15 minutes from `from` for `days` days, using
    /// every due slot (as the worker does), and returns the digests per
    /// local date.
    fn simulate(
        s: &Schedule,
        from: DateTime<Utc>,
        days: i64,
    ) -> BTreeMap<NaiveDate, Vec<DateTime<Utc>>> {
        let mut last = None;
        let mut sent: BTreeMap<NaiveDate, Vec<DateTime<Utc>>> = BTreeMap::new();
        let mut now = from;
        while now < from + Duration::days(days) {
            if let Slot::Due { day, .. } = s.check(last, now) {
                last = Some(day);
                sent.entry(day).or_default().push(now);
            }
            now += Duration::minutes(15);
        }
        sent
    }

    #[test]
    fn defaults_are_9am_on_weekdays_with_quiet_nights() {
        let d = DigestSettings::default();
        assert_eq!((d.time, d.days, d.size), (hm(9, 0), WEEKDAYS, 3));
        assert_eq!((d.quiet_start, d.quiet_end), (hm(21, 0), hm(8, 0)));
        assert_eq!(day_names(d.days), ["mon", "tue", "wed", "thu", "fri"]);
        assert_eq!(timezone(&d), Tz::UTC, "no timezone yet: UTC");
        assert!(Schedule::validate(&d).is_ok());
    }

    #[test]
    fn one_digest_per_local_weekday_at_its_time() {
        let s = Schedule::of(&settings("America/Sao_Paulo"));
        // Monday 2026-10-12 00:00 UTC, two weeks.
        let sent = simulate(&s, utc(2026, 10, 12, 0, 0), 14);
        assert_eq!(sent.len(), 10, "ten weekdays: {sent:?}");
        for (day, times) in &sent {
            assert_eq!(times.len(), 1, "{day}");
            let local = times[0].with_timezone(&s.tz);
            assert_eq!(local.date_naive(), *day);
            assert_eq!(local.time(), hm(9, 0), "09:00 in São Paulo is 12:00 UTC");
            assert!(
                day.weekday().num_days_from_monday() < 5,
                "no weekend digest"
            );
        }
    }

    #[test]
    fn weekends_only_when_chosen() {
        let mut d = settings("Europe/Lisbon");
        let saturday = utc(2026, 10, 17, 8, 0);
        assert_eq!(Schedule::of(&d).check(None, saturday), Slot::DayOff);
        d.days = 0x7f;
        assert!(matches!(
            Schedule::of(&d).check(None, saturday),
            Slot::Due { .. }
        ));
    }

    #[test]
    fn spring_forward_still_one_digest_at_the_first_valid_time() {
        // New York skips 02:00–03:00 on 2027-03-14 (a Sunday).
        let mut d = settings("America/New_York");
        d.days = 0x7f;
        d.time = hm(2, 30);
        d.quiet_start = hm(23, 0);
        d.quiet_end = hm(1, 0);
        let s = Schedule::of(&d);
        let sent = simulate(&s, utc(2027, 3, 12, 0, 0), 5);
        assert_eq!(sent.len(), 5, "{sent:?}");
        assert!(sent.values().all(|t| t.len() == 1));
        let day = date(2027, 3, 14);
        assert_eq!(s.slot_on(day), utc(2027, 3, 14, 7, 0), "03:00 EDT");
        assert_eq!(sent[&day][0], utc(2027, 3, 14, 7, 0));
        // The default 09:00 is simply 09:00 either side of the change.
        let s = Schedule::of(&settings("America/New_York"));
        assert_eq!(s.slot_on(date(2027, 3, 12)), utc(2027, 3, 12, 14, 0));
        assert_eq!(s.slot_on(date(2027, 3, 15)), utc(2027, 3, 15, 13, 0));
    }

    #[test]
    fn fall_back_does_not_send_twice_on_the_long_day() {
        // London repeats 01:00–02:00 on 2026-10-25 (a Sunday): a 25-hour
        // day with the digest time inside the repeated hour.
        let mut d = settings("Europe/London");
        d.days = 0x7f;
        d.time = hm(1, 30);
        d.quiet_start = hm(23, 0);
        d.quiet_end = hm(1, 0);
        let s = Schedule::of(&d);
        let sent = simulate(&s, utc(2026, 10, 23, 0, 0), 5);
        assert!(sent.values().all(|t| t.len() == 1), "{sent:?}");
        assert_eq!(sent.len(), 5);
        // The earlier 01:30 (BST, 00:30 UTC), not the later one.
        assert_eq!(sent[&date(2026, 10, 25)][0], utc(2026, 10, 25, 0, 30));
        // Every 15 minutes across the change in many timezones: never two
        // in one local day, never a slot skipped.
        for tz in [
            "America/Sao_Paulo",
            "America/New_York",
            "Europe/Berlin",
            "Australia/Sydney",
            "Asia/Kolkata",
            "Pacific/Chatham",
        ] {
            let mut d = settings(tz);
            d.days = 0x7f;
            let s = Schedule::of(&d);
            for start in [
                utc(2026, 10, 20, 0, 0),
                utc(2027, 3, 24, 0, 0),
                utc(2027, 4, 1, 0, 0),
            ] {
                let sent = simulate(&s, start, 14);
                assert!(sent.values().all(|t| t.len() == 1), "{tz} {sent:?}");
                assert!(sent.len() >= 13, "{tz}: {}", sent.len());
            }
        }
    }

    #[test]
    fn a_missed_digest_is_not_caught_up() {
        let s = Schedule::of(&settings("UTC"));
        let monday = date(2026, 10, 12);
        assert_eq!(s.check(None, utc(2026, 10, 12, 8, 59)), Slot::NotYet);
        assert!(matches!(
            s.check(None, utc(2026, 10, 12, 9, 0)),
            Slot::Due { .. }
        ));
        assert!(matches!(
            s.check(None, utc(2026, 10, 12, 9, 59)),
            Slot::Due { .. }
        ));
        // The worker was down from 08:50 to 10:05: Monday's digest is gone,
        // and nothing doubles up on Tuesday.
        assert_eq!(s.check(None, utc(2026, 10, 12, 10, 5)), Slot::Missed);
        assert_eq!(s.check(None, utc(2026, 10, 12, 18, 0)), Slot::Missed);
        let tuesday = s.check(None, utc(2026, 10, 13, 9, 0));
        assert_eq!(
            tuesday,
            Slot::Due {
                day: monday.succ_opt().unwrap(),
                at: utc(2026, 10, 13, 9, 0)
            }
        );
        assert_eq!(
            s.check(Some(monday.succ_opt().unwrap()), utc(2026, 10, 13, 9, 15)),
            Slot::AlreadyUsed
        );
    }

    #[test]
    fn pause_holds_digests_and_resumes_without_catching_up() {
        let mut d = settings("UTC");
        d.paused_until = Some(utc(2026, 10, 14, 12, 0));
        let s = Schedule::of(&d);
        assert_eq!(s.check(None, utc(2026, 10, 13, 9, 0)), Slot::Paused);
        // Ends at noon Wednesday: Wednesday's 09:00 is gone.
        assert_eq!(s.check(None, utc(2026, 10, 14, 12, 0)), Slot::Missed);
        assert!(matches!(
            s.check(None, utc(2026, 10, 15, 9, 0)),
            Slot::Due { .. }
        ));
        assert_eq!(
            s.next(None, utc(2026, 10, 13, 9, 0)),
            Some(utc(2026, 10, 15, 9, 0))
        );
        let sent = simulate(&s, utc(2026, 10, 12, 0, 0), 7);
        assert_eq!(
            sent.keys().copied().collect::<Vec<_>>(),
            [date(2026, 10, 15), date(2026, 10, 16)]
        );
    }

    #[test]
    fn quiet_hours_hold_everything_and_refuse_a_digest_time_inside_them() {
        assert!(in_quiet_hours(hm(22, 0), hm(21, 0), hm(8, 0)));
        assert!(in_quiet_hours(hm(7, 59), hm(21, 0), hm(8, 0)));
        assert!(!in_quiet_hours(hm(8, 0), hm(21, 0), hm(8, 0)));
        assert!(!in_quiet_hours(hm(20, 59), hm(21, 0), hm(8, 0)));
        assert!(in_quiet_hours(hm(13, 0), hm(12, 0), hm(14, 0)));
        assert!(!in_quiet_hours(hm(3, 0), hm(0, 0), hm(0, 0)), "equal: none");
        let mut d = settings("UTC");
        d.time = hm(7, 0);
        let error = Schedule::validate(&d).unwrap_err();
        assert!(error.contains("inside your quiet hours"), "{error}");
        d.time = hm(20, 45);
        assert!(Schedule::validate(&d).is_ok());
        // Due at 20:45, but the worker came at 21:05: inside quiet hours,
        // nothing (and later it's missed).
        let s = Schedule::of(&d);
        assert!(matches!(
            s.check(None, utc(2026, 10, 12, 20, 50)),
            Slot::Due { .. }
        ));
        assert_eq!(s.check(None, utc(2026, 10, 12, 21, 5)), Slot::Quiet);
        // A digest composed at 20:45 is never delivered in quiet hours.
        let day = date(2026, 10, 12);
        assert_eq!(s.deliver_by(day, s.slot_on(day)), utc(2026, 10, 12, 21, 0));
        // Without quiet hours: within six hours, and never the next day.
        let mut late = settings("UTC");
        late.time = hm(22, 0);
        late.quiet_start = hm(0, 0);
        late.quiet_end = hm(0, 0);
        let s = Schedule::of(&late);
        assert_eq!(s.deliver_by(day, s.slot_on(day)), utc(2026, 10, 13, 0, 0));
        let s = Schedule::of(&settings("UTC"));
        assert_eq!(s.deliver_by(day, s.slot_on(day)), utc(2026, 10, 12, 15, 0));
        d.days = 0;
        d.time = hm(9, 0);
        assert!(Schedule::validate(&d).is_err());
    }

    #[test]
    fn day_names_round_trip() {
        let names: Vec<String> = ["mon", "wed", "SUN"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let days = days_of(&names).unwrap();
        assert_eq!(day_names(days), ["mon", "wed", "sun"]);
        assert_eq!(days_of(&["funday".to_owned()]), Err("funday".to_owned()));
        assert_eq!(timezone(&settings("Not/AZone")), Tz::UTC);
    }
}
