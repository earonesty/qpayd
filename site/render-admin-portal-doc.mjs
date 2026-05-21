import fs from "node:fs/promises";
import path from "node:path";
import { createHash } from "node:crypto";
import matter from "gray-matter";
import MarkdownIt from "markdown-it";
import markdownItAnchor from "markdown-it-anchor";
import { createHighlighter } from "shiki";

const root = path.resolve(new URL("..", import.meta.url).pathname);
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

const source = await fs.readFile(path.join(root, "docs/admin-portal.release.md"), "utf8");
const parsed = matter(source);
const markdown = fillTemplate(parsed.content, {
  QPAYD_VERSION: qpaydVersion,
  ADMIN_VERSION: adminVersion,
  ADMIN_ASSET_SOURCE: assetSource,
  ADMIN_ASSET_INTEGRITY: integrity,
  ADMIN_ASSET_INTEGRITY_VALUE: integrityValue
});

await fs.mkdir(outDir, { recursive: true });
await fs.writeFile(path.join(outDir, "admin-portal.md"), markdown);
await fs.writeFile(path.join(outDir, "admin-portal.html"), await renderHtml(markdown, parsed.data));

async function readAsset(source) {
  if (/^https?:\/\//.test(source)) return fetchAsset(source);
  if (source.startsWith("file://")) return fs.readFile(new URL(source));
  return fs.readFile(path.isAbsolute(source) ? source : path.resolve(root, source));
}

async function fetchAsset(source) {
  const response = await fetch(source);
  if (!response.ok) {
    throw new Error(`failed to fetch admin asset ${source}: ${response.status}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

function fillTemplate(template, values) {
  return template.replace(/\{\{([A-Z0-9_]+)\}\}/g, (_, key) => {
    if (!(key in values)) throw new Error(`unknown template key ${key}`);
    return values[key];
  });
}

async function renderHtml(markdown, data) {
  const highlighter = await createHighlighter({
    themes: ["github-dark"],
    langs: ["bash", "sh", "toml", "text"]
  });
  const md = new MarkdownIt({
    html: false,
    linkify: true,
    typographer: true,
    highlight(code, lang) {
      const language = highlighter.getLoadedLanguages().includes(lang) ? lang : "text";
      return highlighter.codeToHtml(code, { lang: language, theme: "github-dark" });
    }
  }).use(markdownItAnchor, {
    permalink: markdownItAnchor.permalink.linkInsideHeader({
      symbol: "#",
      placement: "after",
      class: "anchor"
    })
  });
  const css = await siteCss();
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(data.title ?? "Admin Portal")} - qpayd</title>
  <meta name="description" content="${escapeHtml(data.description ?? "")}">
  <style>${css}</style>
</head>
<body>
  <main class="release-doc">
    <article class="docs-article">
      ${md.render(markdown)}
    </article>
  </main>
</body>
</html>`;
}

async function siteCss() {
  const index = await fs.readFile(path.join(root, "site/index.html"), "utf8");
  const match = index.match(/<style>([\s\S]*?)<\/style>/);
  if (!match) throw new Error("site/index.html has no style block");
  return `${match[1]}
    .release-doc {
      width: min(980px, calc(100% - 32px));
      margin: 0 auto;
      padding: 42px 0 72px;
    }

    .docs-article {
      min-width: 0;
      padding: 34px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: var(--panel);
      box-shadow: var(--shadow);
    }

    .docs-article h1 {
      max-width: 760px;
      font-size: clamp(36px, 5vw, 58px);
      line-height: 1;
    }

    .docs-article h2 {
      margin-top: 44px;
      font-size: clamp(24px, 3vw, 34px);
    }

    .docs-article p,
    .docs-article li {
      color: #c8d7d2;
    }

    .docs-article a {
      color: var(--green);
      text-decoration: underline;
      text-underline-offset: 3px;
    }

    .docs-article code:not(pre code) {
      color: var(--green);
      background: #0b1116;
      border: 1px solid var(--line);
      border-radius: 6px;
      padding: 2px 5px;
      font: .92em ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    }

    .docs-article pre {
      overflow: auto;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: #0b1116;
    }

    .docs-article pre code {
      display: block;
      padding: 18px;
      font: 13px/1.65 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    }

    .anchor {
      margin-left: 8px;
      color: var(--muted) !important;
      text-decoration: none !important;
      opacity: .55;
    }

    @media (max-width: 620px) {
      .docs-article {
        padding: 22px;
      }
    }
`;
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    "\"": "&quot;",
    "'": "&#39;"
  })[char]);
}
