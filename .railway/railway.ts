// JobHunt Cloud on Railway (Infrastructure as Code).
//
// Two images (the repository's Dockerfile for the Rust binary,
// apps/web/Dockerfile for the web app), six resources:
//
//   Postgres              the database (shared corpus + private data)
//   api                   `narrow server`: HTTP API and hosted MCP; runs
//                         `narrow migrate` before each deploy
//   web                   the web app (Next.js), which calls `api` over
//                         Railway's private network
//   worker-discovery      cron: reads the sources that are due
//   worker-verification   cron: re-verifies the jobs that matter
//   worker-notify         cron: emails each subscriber's daily digest
//
// Secrets are declared with preserve(): set them once in the dashboard or
// with `railway variable set KEY=… -s api` (and seal them); applying this
// file never overwrites or deletes them. See README.md, "JobHunt Cloud on
// Railway", for every variable and the first deploy.
//
//   npm install            # the `railway` package (package.json)
//   railway link           # the project and environment
//   railway config plan    # review
//   railway config apply

import { defineRailway, github, postgres, preserve, project, service } from "railway/iac";

export default defineRailway((ctx) => {
  const db = postgres("Postgres");
  const source = () => github("bernacle/narrow", { branch: "main" });
  const build = { builder: "DOCKERFILE" as const, dockerfilePath: "Dockerfile" };

  const api = service("api", {
    source: source(),
    build,
    start: "narrow server",
    // Migrations run once per deploy, before the new version starts;
    // concurrent runs are serialized by a Postgres advisory lock.
    preDeploy: "narrow migrate",
    // Deploy-time check: the process answers and Postgres is reachable with
    // the current schema. (Railway does not keep polling it afterwards.)
    healthcheck: "/ready",
    healthcheckTimeout: 120,
    env: {
      DATABASE_URL: db.env.DATABASE_URL,
      JOBHUNT_DB_MAX_CONNECTIONS: "10",
      // A fixed port, so `web` can reach it on the private network.
      PORT: "8080",
      // Secrets and per-environment values, set by hand:
      JOBHUNT_ENCRYPTION_KEYS: preserve(),
      JOBHUNT_PUBLIC_URL: preserve(),
      JOBHUNT_OIDC_ISSUER: preserve(),
      JOBHUNT_OIDC_AUDIENCE: preserve(),
      JOBHUNT_OIDC_CLI_CLIENT_ID: preserve(),
      JOBHUNT_OIDC_AUDIENCE_PARAMETER: preserve(),
      JOBHUNT_ALLOWED_ORIGINS: preserve(),
      // Links in emails, and the address confirmation email the API sends.
      JOBHUNT_WEB_URL: preserve(),
      JOBHUNT_EMAIL_PROVIDER: preserve(),
      JOBHUNT_RESEND_API_KEY: preserve(),
      JOBHUNT_EMAIL_FROM: preserve(),
    },
    deploy: {
      restartPolicyType: "ON_FAILURE",
      restartPolicyMaxRetries: 10,
      // SIGTERM, then this long to finish requests in flight.
      drainingSeconds: 30,
      overlapSeconds: 20,
    },
  });

  const discovery = service("worker-discovery", {
    source: source(),
    build,
    start: "narrow worker discovery",
    env: {
      DATABASE_URL: db.env.DATABASE_URL,
      JOBHUNT_DB_MAX_CONNECTIONS: "4",
    },
    deploy: {
      // Every 30 minutes (UTC); each run reads only the sources that are
      // due (active sources every 3 h, others every 12 h, failing ones
      // back off). Railway skips a run while the previous one is going.
      cronSchedule: "7,37 * * * *",
      restartPolicyType: "NEVER",
    },
  });

  const verification = service("worker-verification", {
    source: source(),
    build,
    start: "narrow worker verification",
    env: {
      DATABASE_URL: db.env.DATABASE_URL,
      JOBHUNT_DB_MAX_CONNECTIONS: "4",
    },
    deploy: {
      cronSchedule: "22 * * * *",
      restartPolicyType: "NEVER",
    },
  });

  const notify = service("worker-notify", {
    source: source(),
    build,
    start: "narrow worker notify",
    env: {
      DATABASE_URL: db.env.DATABASE_URL,
      JOBHUNT_DB_MAX_CONNECTIONS: "4",
      // Reads private data (the address, rankings) and sends email: the
      // same keys as `api`, and the email provider.
      JOBHUNT_ENCRYPTION_KEYS: preserve(),
      JOBHUNT_WEB_URL: preserve(),
      JOBHUNT_EMAIL_PROVIDER: preserve(),
      JOBHUNT_RESEND_API_KEY: preserve(),
      JOBHUNT_EMAIL_FROM: preserve(),
    },
    deploy: {
      // Every 15 minutes, after discovery (7, 37) and verification (22)
      // had a chance to find and check new jobs. Each account gets at
      // most one digest per local day, at its own digest time (the run
      // nearest after it composes it).
      cronSchedule: "12,27,42,57 * * * *",
      restartPolicyType: "NEVER",
    },
  });

  const web = service("web", {
    source: source(),
    build: { builder: "DOCKERFILE" as const, dockerfilePath: "apps/web/Dockerfile", watchPatterns: ["apps/web/**"] },
    healthcheck: "/healthz",
    env: {
      // Server to server, over the private network (never exposed to the
      // browser).
      JOBHUNT_API_URL: "http://${{api.RAILWAY_PRIVATE_DOMAIN}}:8080",
      PORT: "3000",
      // Set by hand:
      JOBHUNT_API_PUBLIC_URL: preserve(),
      JOBHUNT_WEB_URL: preserve(),
      JOBHUNT_WEB_SESSION_SECRET: preserve(),
      JOBHUNT_WEB_OIDC_CLIENT_ID: preserve(),
      JOBHUNT_WEB_OIDC_CLIENT_SECRET: preserve(),
    },
    deploy: {
      restartPolicyType: "ON_FAILURE",
      restartPolicyMaxRetries: 10,
      drainingSeconds: 15,
      overlapSeconds: 15,
    },
  });

  return project("jobhunt", { resources: [db, api, web, discovery, verification, notify] });
});
