import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(new URL("../src/index.js", import.meta.url), "utf8");

test("checkout modal keeps supported styling classes", () => {
  [
    "qpayd-modal-root",
    "qpayd-backdrop",
    "qpayd-modal",
    "qpayd-head",
    "qpayd-summary",
    "qpayd-tabs",
    "qpayd-panel",
    "qpayd-value",
    "qpayd-actions",
    "qpayd-note",
    "qpayd-error",
    "qpayd-error-panel",
  ].forEach((className) => {
    assert.ok(source.includes(className), `missing supported class ${className}`);
  });
});

test("checkout modal keeps supported data attributes", () => {
  [
    "data-qpayd-close",
    "data-qpayd-copy",
    "data-qpayd-error",
    "data-qpayd-expiry",
    "data-qpayd-fiat",
    "data-qpayd-method",
    "data-qpayd-panel",
    "data-qpayd-qr",
    "data-qpayd-sats",
    "data-qpayd-status",
    "data-qpayd-uri",
    "data-qpayd-value",
    "data-status",
  ].forEach((attributeName) => {
    assert.ok(source.includes(attributeName), `missing supported attribute ${attributeName}`);
  });
});

test("checkout modal supports merchant host class names", () => {
  assert.match(source, /options\.className/);
  assert.match(source, /classList\.add/);
});
