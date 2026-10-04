//! Role breadth, read from a posting's own responsibilities
//! (`docs/role-breadth-fit-experiment.md`): how concentrated the actual
//! work is, without naming specialties. An offline experiment, not wired
//! into ranking.
//!
//! The reading uses generic properties only:
//!
//! * **duties**: the lines under a responsibilities heading or lead ("What
//!   you'll do", "In this role, you'll:"), else the sentences addressed to
//!   the person ("you'll …"). Requirements are not duties.
//! * **concentration**: whether one content term (not a function word, a
//!   generic work verb, the role family's own name or the company's) recurs
//!   in most duties.
//! * **breadth language**: the posting saying the role is generalist ("a
//!   generalist group", "across the stack", "a wide range of systems").
//! * **depth language**: duties at a lower layer ("internals", "inner
//!   workings", "low-level", "primitives"), or one of the depth specialties
//!   Narrow already reads.
//!
//! The vocabularies below are about language (stop words, generic verbs,
//! breadth and depth wording), never about specialties.

use std::collections::BTreeMap;

use jobhunt_profile::words::words;
use jobhunt_ranking::facets::work::specialties_in;
use serde::{Deserialize, Serialize};

/// How concentrated the work is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Breadth {
    Broad,
    Focused,
    DeepSpecialist,
    Unclear,
}

impl Breadth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Broad => "broad",
            Self::Focused => "focused",
            Self::DeepSpecialist => "deep_specialist",
            Self::Unclear => "unclear",
        }
    }

    /// Focused or deep specialist.
    pub fn narrow(self) -> bool {
        matches!(self, Self::Focused | Self::DeepSpecialist)
    }
}

/// A breadth reading with what it rests on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BreadthReading {
    pub breadth: Breadth,
    /// The recurring term, when one dominates the duties.
    pub concern: Option<String>,
    /// Duties containing it, of all duties.
    pub share: (usize, usize),
    /// Breadth wording found ("generalist", "across the stack").
    pub breadth_cue: Option<String>,
    /// Depth wording or specialty found in the duties.
    pub depth_cue: Option<String>,
    /// How the duties were found.
    pub duties_from: String,
}

/// A term recurring in at least this share of the duties concentrates the
/// work.
pub const CONCENTRATION: f64 = 0.5;
/// …and in at least this many duties.
pub const MIN_RECURRENCE: usize = 3;
/// Duties needed to call a role broad from diversity alone.
pub const MIN_DUTIES_BROAD: usize = 4;

/// Headings over what the person will do.
const DUTY_HEADINGS: [&str; 18] = [
    "responsibilities",
    "key responsibilities",
    "your responsibilities",
    "core responsibilities",
    "what you ll do",
    "what you will do",
    "what you ll be doing",
    "what you will be doing",
    "what you ll work on",
    "what you will work on",
    "what you ll be responsible for",
    "what you will be responsible for",
    "what you ll own",
    "what you ll achieve",
    "in this role",
    "in this role you will",
    "your role",
    "you will",
];

/// Headings that end a list of duties (requirements, the person, the
/// company, benefits). Any other heading inside the list is a subheading
/// of the duties ("DETECTION ENGINEERING").
const END_HEADINGS: [&str; 24] = [
    "requirement",
    "qualification",
    "about you",
    "who you are",
    "what you bring",
    "what you ll bring",
    "what you ll need",
    "what you need",
    "what we re looking for",
    "what makes you",
    "you have",
    "you bring",
    "you might be",
    "you may be",
    "you should",
    "you will be a great fit",
    "nice to have",
    "bonus",
    "benefit",
    "about us",
    "about the team",
    "why join",
    "what we offer",
    "compensation",
];

/// Function words and generic work words: never a concern.
const STOP: &[&str] = &[
    // function words
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "from",
    "into",
    "our",
    "your",
    "you",
    "will",
    "are",
    "our",
    "its",
    "their",
    "they",
    "them",
    "who",
    "what",
    "when",
    "where",
    "which",
    "how",
    "all",
    "any",
    "each",
    "every",
    "both",
    "more",
    "most",
    "such",
    "other",
    "than",
    "then",
    "also",
    "can",
    "may",
    "able",
    "well",
    "new",
    "way",
    "ways",
    "use",
    "using",
    "used",
    "like",
    "including",
    "across",
    "within",
    "between",
    "through",
    "over",
    "about",
    "help",
    "helping",
    "make",
    "making",
    "get",
    "set",
    "via",
    "per",
    "own",
    "owning",
    "owned",
    "ownership",
    // generic work verbs and nouns
    "build",
    "building",
    "built",
    "design",
    "designing",
    "develop",
    "developing",
    "development",
    "maintain",
    "maintaining",
    "improve",
    "improving",
    "drive",
    "driving",
    "lead",
    "leading",
    "work",
    "working",
    "ensure",
    "ensuring",
    "support",
    "supporting",
    "collaborate",
    "collaborating",
    "partner",
    "partnering",
    "create",
    "creating",
    "deliver",
    "delivering",
    "implement",
    "implementing",
    "write",
    "writing",
    "ship",
    "shipping",
    "operate",
    "operating",
    "manage",
    "managing",
    "define",
    "defining",
    "contribute",
    "contributing",
    "scale",
    "scaling",
    "scalable",
    "evolve",
    "evolving",
    "identify",
    "identifying",
    "mentor",
    "mentoring",
    "review",
    "reviews",
    "solve",
    "solving",
    "plan",
    "planning",
    "run",
    "running",
    "enable",
    "enabling",
    "team",
    "teams",
    "engineer",
    "engineers",
    "engineering",
    "company",
    "companies",
    "customer",
    "customers",
    "user",
    "users",
    "people",
    "stakeholder",
    "stakeholders",
    "partners",
    "system",
    "systems",
    "service",
    "services",
    "solution",
    "solutions",
    "tool",
    "tools",
    "tooling",
    "product",
    "products",
    "feature",
    "features",
    "project",
    "projects",
    "process",
    "processes",
    "quality",
    "technical",
    "technology",
    "technologies",
    "high",
    "best",
    "key",
    "core",
    "critical",
    "complex",
    "large",
    "small",
    "strong",
    "reliable",
    "reliability",
    "performance",
    "performant",
    "secure",
    "code",
    "software",
    "experience",
    "experiences",
    "practice",
    "practices",
    "standard",
    "standards",
    "decision",
    "decisions",
    "problem",
    "problems",
    "roadmap",
    "strategy",
    "direction",
    "impact",
    "production",
    "time",
    "level",
    // role families: their own name says nothing about concentration
    "backend",
    "frontend",
    "front",
    "end",
    "full",
    "stack",
    "fullstack",
    "platform",
    "platforms",
    "infrastructure",
    "infra",
    "mobile",
    "web",
    "data",
    "security",
    "cloud",
    "application",
    "applications",
    "app",
    "apps",
    "api",
    "apis",
];

/// The posting saying the role is broad.
const BREADTH_CUES: [&str; 12] = [
    "generalist",
    "generalists",
    "across the stack",
    "across the full stack",
    "full stack systems role",
    "wide range of",
    "wide surface",
    "many hats",
    "collection of hats",
    "spectrum of areas",
    "every part of",
    "any part of",
];

/// Duties at a lower layer.
const DEPTH_CUES: [&str; 7] = [
    "internals",
    "inner workings",
    "low level",
    "primitives",
    "kernel",
    "genuine depth",
    "first principles",
];

fn normal(word: &str) -> Option<String> {
    let w = word.to_lowercase();
    if w.len() < 3 || !w.chars().all(char::is_alphabetic) || STOP.contains(&w.as_str()) {
        return None;
    }
    // A plural is its singular.
    let w = if w.len() > 4 && w.ends_with('s') && !w.ends_with("ss") {
        w[..w.len() - 1].to_owned()
    } else {
        w
    };
    (!STOP.contains(&w.as_str())).then_some(w)
}

fn key(text: &str) -> String {
    // "you’ll" is "you'll".
    words(&text.replace(['’', '‘'], "'"))
        .iter()
        .map(|w| w.lower.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a line opens a list of duties (`Some(true)`), closes one
/// (`Some(false)`), or neither.
fn duty_marker(line: &str) -> Option<bool> {
    let trimmed = line.trim();
    let bullet = trimmed.starts_with(['-', '*', '•', '–']);
    if trimmed.is_empty() || bullet {
        return None;
    }
    let k = key(trimmed.trim_end_matches([':', '?']));
    let n = k.split(' ').filter(|w| !w.is_empty()).count();
    let heading = n <= 7 && !trimmed.ends_with('.');
    let lead = trimmed.ends_with(':');
    let duty = DUTY_HEADINGS.iter().any(|h| k.starts_with(h))
        || ((lead || trimmed.ends_with('?'))
            && (k.contains("you ll") || k.contains("you will") || k.contains("will you")));
    let ends = END_HEADINGS.iter().any(|h| k.starts_with(h));
    if (heading || lead) && duty {
        Some(true)
    } else if (heading || lead) && ends {
        Some(false)
    } else {
        None
    }
}

/// The duties of a description, and how they were found.
pub fn duties(description: &str) -> (Vec<String>, &'static str) {
    let mut out = Vec::new();
    let mut open = false;
    for line in description.lines() {
        if let Some(o) = duty_marker(line) {
            open = o;
            continue;
        }
        let text = line.trim().trim_start_matches(['-', '*', '•', '–', ' ']);
        if open && !text.is_empty() {
            out.push(text.to_owned());
        }
    }
    if !out.is_empty() {
        return (out, "responsibilities");
    }
    // No responsibilities section: the sentences addressed to the person.
    let addressed: Vec<String> = description
        .lines()
        .flat_map(|l| l.split(". "))
        .map(str::trim)
        .filter(|s| {
            let k = key(s);
            k.contains("you ll") || k.contains("you will")
        })
        .map(str::to_owned)
        .collect();
    (addressed, "addressed")
}

fn find_cue<'a>(texts: &[String], cues: &'a [&'a str]) -> Option<&'a str> {
    texts.iter().find_map(|t| {
        let k = format!(" {} ", key(t));
        cues.iter().find(|c| k.contains(&format!(" {c} "))).copied()
    })
}

/// Reads a posting's breadth from its title and description. `company`
/// words never count as a concern.
pub fn read(title: &str, description: &str, company: &str) -> BreadthReading {
    let (duties, duties_from) = duties(description);
    let company_words: Vec<String> = words(company).iter().map(|w| w.lower.clone()).collect();
    // Each term counts once per duty.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for d in &duties {
        let mut seen: Vec<String> = Vec::new();
        for w in words(d) {
            let Some(t) = normal(&w.original) else {
                continue;
            };
            if company_words.contains(&t) || seen.contains(&t) {
                continue;
            }
            seen.push(t.clone());
            *counts.entry(t).or_default() += 1;
        }
    }
    let n = duties.len();
    let top = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(t, c)| (t.clone(), *c));
    let concentrated = top.as_ref().is_some_and(|(_, c)| {
        n >= MIN_RECURRENCE && *c >= MIN_RECURRENCE && (*c as f64) / (n as f64) >= CONCENTRATION
    });
    // Breadth wording about the role: in the duties, or a sentence about
    // the role ("This is a flexible, full-stack systems role").
    let mut about_role: Vec<String> = duties.clone();
    about_role.extend(description.lines().map(str::to_owned).filter(|l| {
        let k = format!(" {} ", key(l));
        k.contains(" role ") || k.contains(" you ") || k.contains(" team ")
    }));
    let breadth_cue = find_cue(&about_role, &BREADTH_CUES).map(str::to_owned);
    let mut texts: Vec<String> = duties.clone();
    texts.push(title.to_owned());
    let depth_word = find_cue(&texts, &DEPTH_CUES).map(str::to_owned);
    let duty_sentences: Vec<(String, Vec<jobhunt_profile::words::Word>)> =
        duties.iter().map(|d| (d.clone(), words(d))).collect();
    let specialty = specialties_in(title, &duty_sentences)
        .first()
        .map(|s| s.specialty.as_str().to_owned());
    let depth_cue = specialty.or(depth_word);
    let breadth = match (concentrated, &breadth_cue, &depth_cue) {
        (true, _, Some(_)) => Breadth::DeepSpecialist,
        (_, Some(_), _) => Breadth::Broad,
        (true, None, None) => Breadth::Focused,
        (false, None, _) if n >= MIN_DUTIES_BROAD => Breadth::Broad,
        (false, None, _) => Breadth::Unclear,
    };
    BreadthReading {
        breadth,
        concern: top
            .as_ref()
            .filter(|_| concentrated)
            .map(|(t, _)| t.clone()),
        share: (top.map_or(0, |(_, c)| c), n),
        breadth_cue,
        depth_cue,
        duties_from: duties_from.to_owned(),
    }
}

/// A human breadth label (see the fixture's conventions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub breadth: String,
    #[serde(default)]
    pub also: Vec<String>,
}

/// Phase 1 metrics.
#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    pub scored: usize,
    pub out_of_scope: usize,
    /// annotation -> reading -> count
    pub confusion: BTreeMap<String, BTreeMap<String, usize>>,
    pub narrow_marked: usize,
    pub narrow_marked_correct: usize,
    pub narrow_annotated: usize,
    pub narrow_annotated_found: usize,
    pub broad_annotated: usize,
    pub broad_kept: usize,
    pub deep_annotated: usize,
    pub deep_called_broad: usize,
    pub unclear: usize,
    pub strict_agreement: usize,
    pub gates: Vec<(&'static str, bool)>,
    pub pass: bool,
}

/// Scores readings against annotations. Narrowness precision accepts a
/// narrow label anywhere in the annotation (its `also` included);
/// recall and broad preservation use the first label.
pub fn evaluate(rows: &[(Annotation, Breadth)]) -> Metrics {
    let narrow_label = |s: &str| s == "focused" || s == "deep_specialist";
    let mut m = Metrics {
        scored: 0,
        out_of_scope: 0,
        confusion: BTreeMap::new(),
        narrow_marked: 0,
        narrow_marked_correct: 0,
        narrow_annotated: 0,
        narrow_annotated_found: 0,
        broad_annotated: 0,
        broad_kept: 0,
        deep_annotated: 0,
        deep_called_broad: 0,
        unclear: 0,
        strict_agreement: 0,
        gates: Vec::new(),
        pass: false,
    };
    for (a, got) in rows {
        if a.breadth == "out_of_scope" {
            m.out_of_scope += 1;
            continue;
        }
        m.scored += 1;
        *m.confusion
            .entry(a.breadth.clone())
            .or_default()
            .entry(got.as_str().to_owned())
            .or_default() += 1;
        m.strict_agreement += usize::from(a.breadth == got.as_str());
        m.unclear += usize::from(*got == Breadth::Unclear);
        if got.narrow() {
            m.narrow_marked += 1;
            m.narrow_marked_correct +=
                usize::from(narrow_label(&a.breadth) || a.also.iter().any(|x| narrow_label(x)));
        }
        if narrow_label(&a.breadth) {
            m.narrow_annotated += 1;
            m.narrow_annotated_found += usize::from(got.narrow());
        }
        if a.breadth == "broad" {
            m.broad_annotated += 1;
            m.broad_kept += usize::from(matches!(got, Breadth::Broad | Breadth::Unclear));
        }
        if a.breadth == "deep_specialist" {
            m.deep_annotated += 1;
            m.deep_called_broad += usize::from(*got == Breadth::Broad);
        }
    }
    let rate = |a: usize, b: usize| if b == 0 { 0.0 } else { a as f64 / b as f64 };
    m.gates = vec![
        (
            "narrowness precision >= 90%",
            m.narrow_marked > 0 && rate(m.narrow_marked_correct, m.narrow_marked) >= 0.90,
        ),
        (
            "narrowness recall >= 70%",
            rate(m.narrow_annotated_found, m.narrow_annotated) >= 0.70,
        ),
        (
            "broad preservation >= 95%",
            rate(m.broad_kept, m.broad_annotated) >= 0.95,
        ),
        ("no deep specialist read as broad", m.deep_called_broad == 0),
    ];
    m.pass = m.gates.iter().all(|(_, ok)| *ok);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duties_come_from_the_responsibilities_section() {
        let (d, from) = duties(
            "About us\nWe build things.\nWhat you'll do\n- Own billing\n- Own invoices\nRequirements\n- Go",
        );
        assert_eq!(from, "responsibilities");
        assert_eq!(d, ["Own billing", "Own invoices"]);
    }

    #[test]
    fn a_recurring_concern_is_focused_and_variety_is_broad() {
        let focused = read(
            "Engineer",
            "What you'll do\n- Find cost savings across our cloud\n- Build cost attribution\n\
             - Automate cost policy\n- Report cost trends to finance",
            "Acme",
        );
        assert_eq!(focused.breadth, Breadth::Focused, "{focused:?}");
        assert_eq!(focused.concern.as_deref(), Some("cost"));
        let broad = read(
            "Engineer",
            "What you'll do\n- Ship the billing API\n- Improve search ranking\n\
             - Add realtime collaboration to the editor\n- Profile list rendering",
            "Acme",
        );
        assert_eq!(broad.breadth, Breadth::Broad, "{broad:?}");
        let unclear = read("Engineer", "We are hiring.", "Acme");
        assert_eq!(unclear.breadth, Breadth::Unclear);
    }

    #[test]
    fn depth_with_concentration_is_deep() {
        let deep = read(
            "Engineer",
            "What you'll do\n- Work on the kernel I/O path of the block storage layer\n\
             - Tune block storage latency\n- Debug block storage failures",
            "Acme",
        );
        assert_eq!(deep.breadth, Breadth::DeepSpecialist, "{deep:?}");
    }
}
