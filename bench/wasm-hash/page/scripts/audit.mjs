#!/usr/bin/env node
// Fails the page job on high or critical advisories unless audit-allow.json accepts them by GHSA id.
// Every acceptance names an owner and a recheck date so it cannot go stale without notice.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const pageDir = dirname(dirname(fileURLToPath(import.meta.url)));
const allowlistPath = join(pageDir, "audit-allow.json");
const severityRank = { info: 0, low: 1, moderate: 2, high: 3, critical: 4 };
const ghsaPattern = /GHSA-[0-9a-z]{4}-[0-9a-z]{4}-[0-9a-z]{4}/i;
const ghsaIdPattern = /^GHSA-[0-9a-z]{4}-[0-9a-z]{4}-[0-9a-z]{4}$/i;

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function isDate(value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const date = new Date(`${value}T00:00:00Z`);
  return !Number.isNaN(date.valueOf()) && date.toISOString().slice(0, 10) === value;
}

function readAllowlist() {
  let entries;
  try {
    entries = JSON.parse(readFileSync(allowlistPath, "utf8"));
  } catch (error) {
    throw new Error(`cannot read ${allowlistPath}: ${error.message}`);
  }
  if (!Array.isArray(entries)) throw new Error("audit-allow.json must be a JSON array");
  const seen = new Set();
  for (const [index, entry] of entries.entries()) {
    const at = `audit-allow.json[${index}]`;
    if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
      throw new Error(`${at} must be an object`);
    }
    for (const field of ["id", "package", "reason", "approved_by", "recheck_by"]) {
      if (typeof entry[field] !== "string" || entry[field].trim() === "") {
        throw new Error(`${at}.${field} must be a non-empty string`);
      }
    }
    if (!ghsaIdPattern.test(entry.id)) throw new Error(`${at}.id is not a GHSA id: ${entry.id}`);
    if (!isDate(entry.recheck_by)) throw new Error(`${at}.recheck_by is not a YYYY-MM-DD date`);
    const key = entry.id.toLowerCase();
    if (seen.has(key)) throw new Error(`${at}.id repeats ${entry.id}`);
    seen.add(key);
  }
  return entries;
}

function runAudit() {
  const windows = process.platform === "win32";
  const command = windows ? (process.env.ComSpec ?? "cmd.exe") : "npm";
  const args = windows ? ["/d", "/s", "/c", "npm audit --json"] : ["audit", "--json"];
  const result = spawnSync(command, args, {
    cwd: pageDir,
    encoding: "utf8",
    env: { ...process.env, NO_COLOR: "1", npm_config_update_notifier: "false" },
  });
  if (result.error) throw new Error(`cannot run npm audit: ${result.error.message}`);
  const stdout = (result.stdout ?? "").trim();
  if (stdout === "") {
    const stderr = (result.stderr ?? "").trim();
    throw new Error(
      `npm audit wrote no JSON (exit ${result.status})${stderr ? `: ${stderr}` : ""}`,
    );
  }
  let report;
  try {
    report = JSON.parse(stdout);
  } catch {
    throw new Error(`npm audit did not write JSON: ${stdout.slice(0, 200)}`);
  }
  if (report.error) {
    throw new Error(
      `npm audit failed: ${report.error.summary ?? report.error.code ?? "unknown error"}`,
    );
  }
  return report;
}

function advisories(report) {
  const found = new Map();
  for (const [packageName, vulnerability] of Object.entries(report.vulnerabilities ?? {})) {
    for (const via of vulnerability.via ?? []) {
      if (typeof via !== "object" || via === null) continue;
      const match = ghsaPattern.exec(via.url ?? "");
      if (!match) {
        if (via.severity === "high" || via.severity === "critical") {
          throw new Error(
            `npm reported a ${via.severity} advisory for ${packageName} without a GHSA url`,
          );
        }
        continue;
      }
      const id = match[0].replace(/^ghsa/i, "GHSA");
      const advisory = {
        id,
        key: id.toLowerCase(),
        package: via.name ?? packageName,
        severity: via.severity ?? vulnerability.severity,
        title: via.title ?? "(no title)",
        range: via.range ?? "unknown",
      };
      const previous = found.get(advisory.key);
      if (!previous || severityRank[advisory.severity] > severityRank[previous.severity]) {
        found.set(advisory.key, advisory);
      }
    }
  }
  return found;
}

function main() {
  const allowlist = readAllowlist();
  const accepted = new Map(allowlist.map((entry) => [entry.id.toLowerCase(), entry]));
  const matched = new Set();
  const report = runAudit();
  const found = advisories(report);
  const today = new Date().toISOString().slice(0, 10);

  const counts = report.metadata?.vulnerabilities ?? {};
  if ((counts.high ?? 0) + (counts.critical ?? 0) > 0 && found.size === 0) {
    throw new Error("npm audit reports high or critical vulnerabilities but no GHSA advisory");
  }

  for (const entry of allowlist) {
    if (entry.recheck_by < today) {
      fail(
        `allowlist entry ${entry.id} for ${entry.package} passed its recheck_by ${entry.recheck_by}; ` +
          "recheck the advisory and renew the date or remove the entry",
      );
    }
  }

  for (const advisory of found.values()) {
    const entry = accepted.get(advisory.key);
    if (entry) matched.add(advisory.key);
    const label = `${advisory.id} (${advisory.severity}) in ${advisory.package}`;
    if (entry && (advisory.severity === "high" || advisory.severity === "critical")) {
      console.log(`ACCEPTED: ${label} - recheck by ${entry.recheck_by}`);
    } else if (advisory.severity === "high" || advisory.severity === "critical") {
      fail(`${label} - ${advisory.title} (${advisory.range}); not in audit-allow.json`);
    } else {
      console.log(`LISTED: ${label} - ${advisory.title}`);
    }
  }

  for (const entry of allowlist) {
    if (!matched.has(entry.id.toLowerCase())) {
      console.log(
        `NOTE: allowlist entry ${entry.id} for ${entry.package} matches no current advisory; ` +
          "remove it once the package is fixed",
      );
    }
  }

  if (process.exitCode !== 1) console.log("audit ok");
}

try {
  main();
} catch (error) {
  fail(error.message);
}
