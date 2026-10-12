// The end-to-end stack's addresses and environment, shared by the stack
// launcher (stack.mjs) and the tests (which run workers themselves).
import { fileURLToPath } from "node:url";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));

export const PORTS = { web: 3100, api: 4020, fixtures: 4010 };
export const WEB_URL = `http://127.0.0.1:${PORTS.web}`;
export const API_URL = `http://127.0.0.1:${PORTS.api}`;
export const FIXTURES_URL = `http://127.0.0.1:${PORTS.fixtures}`;
export const STATE_DIR = here("../.e2e/");
export const MAIL_FILE = `${STATE_DIR}mail.jsonl`;
export const JOBHUNT_BIN = process.env.JOBHUNT_BIN ?? here("../../../target/debug/narrow");

/** The Postgres server; the stack uses a fresh `jobhunt_e2e` database on it. */
export const ADMIN_DATABASE_URL =
  process.env.JOBHUNT_E2E_DATABASE_URL ?? "postgres://jobhunt:jobhunt@127.0.0.1:5432/jobhunt";

export function databaseUrl() {
  const url = new URL(ADMIN_DATABASE_URL);
  url.pathname = "/jobhunt_e2e";
  return url.toString();
}

/** What every JobHunt Cloud process of the stack runs with. */
export function cloudEnv() {
  return {
    ...process.env,
    DATABASE_URL: databaseUrl(),
    JOBHUNT_ENV: "e2e",
    JOBHUNT_CONFIG: here("cloud.toml"),
    JOBHUNT_ENCRYPTION_KEYS: "e2e:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    JOBHUNT_AUTH_MODE: "dev",
    JOBHUNT_AUTH_DEV_SECRET: "e2e-development-secret-0123456789abcdef",
    JOBHUNT_PUBLIC_URL: API_URL,
    JOBHUNT_WEB_URL: WEB_URL,
    JOBHUNT_BIND: `127.0.0.1:${PORTS.api}`,
    JOBHUNT_EMAIL_PROVIDER: "file",
    JOBHUNT_EMAIL_FILE: MAIL_FILE,
    JOBHUNT_EMAIL_FROM: "JobHunt <notifications@jobhunt.test>",
    JOBHUNT_DISCOVERY_ENDPOINT: FIXTURES_URL,
    JOBHUNT_VERIFY_ENDPOINT: FIXTURES_URL,
    JOBHUNT_GITHUB_ENDPOINT: `${FIXTURES_URL}/github`,
    JOBHUNT_LOG_FORMAT: "text",
    // Nothing in the stack may reach the internet.
    HTTP_PROXY: "http://127.0.0.1:9",
    HTTPS_PROXY: "http://127.0.0.1:9",
    NO_PROXY: "localhost,127.0.0.1,::1",
  };
}

export function webEnv() {
  return {
    ...process.env,
    JOBHUNT_API_URL: API_URL,
    JOBHUNT_API_PUBLIC_URL: API_URL,
    JOBHUNT_WEB_URL: WEB_URL,
    JOBHUNT_WEB_AUTH_MODE: "dev",
    JOBHUNT_WEB_SESSION_SECRET: "e2e-session-secret-0123456789abcdef-0123456789",
    PORT: String(PORTS.web),
    HOSTNAME: "127.0.0.1",
  };
}
