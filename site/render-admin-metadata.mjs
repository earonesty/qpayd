import fs from "node:fs/promises";
import path from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const outDir = path.resolve(process.env.OUT_DIR ?? path.join(root, "dist"));
const qpaydVersion = process.env.QPAYD_VERSION ?? process.env.GITHUB_REF_NAME ?? "dev";
const adminVersion = (process.env.ADMIN_VERSION ?? qpaydVersion).replace(/^v/, "");
const assetSource =
  process.env.ADMIN_ASSET_SOURCE ??
  `https://cdn.jsdelivr.net/npm/@qpayd/admin@${adminVersion}/src/index.js`;
const assetIntegritySource = process.env.ADMIN_ASSET_INTEGRITY_SOURCE ?? assetSource;

const assetBytes = await readAsset(assetIntegritySource);
const integrityValue = createHash("sha384").update(assetBytes).digest("base64");
const integrity = `sha384-${integrityValue}`;

const metadata = {
  qpayd_version: qpaydVersion,
  admin_version: adminVersion,
  asset_source: assetSource,
  asset_integrity: integrity,
  asset_integrity_value: integrityValue,
  generated_at: new Date().toISOString()
};

await fs.mkdir(outDir, { recursive: true });
await fs.writeFile(path.join(outDir, "admin-portal.json"), `${JSON.stringify(metadata, null, 2)}\n`);

async function readAsset(source) {
  if (/^https?:\/\//.test(source)) return fetchAsset(source);
  if (source.startsWith("file://")) return fs.readFile(new URL(source));
  return fs.readFile(path.isAbsolute(source) ? source : path.resolve(root, source));
}

async function fetchAsset(source) {
  const response = await fetch(source, {
    signal: AbortSignal.timeout(15000)
  });
  if (!response.ok) {
    throw new Error(`failed to fetch admin asset ${source}: ${response.status}`);
  }
  return Buffer.from(await response.arrayBuffer());
}
