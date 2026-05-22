import fs from "node:fs/promises";
import path from "node:path";
import matter from "gray-matter";
import MarkdownIt from "markdown-it";
import markdownItAnchor from "markdown-it-anchor";
import { createHighlighter } from "shiki";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const docsDir = path.join(root, "docs");
const siteDir = path.join(root, "site");
const outDir = path.resolve(process.env.SITE_OUT_DIR ?? path.join(root, "public"));
const docsOutDir = path.join(outDir, "docs");

const highlighter = await createHighlighter({
  themes: ["github-dark"],
  langs: ["bash", "sh", "toml", "json", "html", "text"]
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

const files = (await fs.readdir(docsDir))
  .filter((file) => file.endsWith(".md") && !file.endsWith(".release.md"))
  .sort();
const pages = [];

for (const file of files) {
  const source = await fs.readFile(path.join(docsDir, file), "utf8");
  const parsed = matter(source);
  const slug = file === "index.md" ? "index" : file.replace(/\.md$/, "");
  pages.push({
    slug,
    file,
    title: parsed.data.title ?? titleFromSlug(slug),
    description: parsed.data.description ?? "",
    order: Number(parsed.data.order ?? 1000),
    html: md.render(parsed.content)
  });
}

pages.sort((left, right) => left.order - right.order || left.title.localeCompare(right.title));

await fs.mkdir(docsOutDir, { recursive: true });
const css = await siteCss();

for (const page of pages) {
  await fs.writeFile(path.join(docsOutDir, `${page.slug}.html`), renderPage(page, pages, css));
}

function renderPage(page, allPages, css) {
  const nav = allPages
    .map((item) => {
      const current = item.slug === page.slug ? ` aria-current="page"` : "";
      return `<a href="./${item.slug}.html"${current}>${escapeHtml(item.title)}</a>`;
    })
    .join("\n");

  const previous = allPages[allPages.indexOf(page) - 1];
  const next = allPages[allPages.indexOf(page) + 1];
  const pageScript = page.slug === "admin-portal" ? adminPortalScript() : "";

  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(page.title)} - qpayd docs</title>
  <meta name="description" content="${escapeHtml(page.description)}">
  <style>${css}</style>
</head>
<body>
  <header class="shell nav">
    <a class="brand" href="../" aria-label="qpayd home"><span class="mark">q</span><span>qpayd</span></a>
    <nav class="links" aria-label="Primary">
      <a href="./index.html">Docs</a>
      <a href="../#features">Features</a>
      <a href="https://github.com/earonesty/qpayd">GitHub</a>
    </nav>
  </header>
  <main class="shell docs-layout">
    <aside class="docs-nav" aria-label="Docs">${nav}</aside>
    <article class="docs-article">
      ${page.html}
      <nav class="docs-pager" aria-label="Docs pagination">
        ${previous ? `<a href="./${previous.slug}.html">Previous: ${escapeHtml(previous.title)}</a>` : "<span></span>"}
        ${next ? `<a href="./${next.slug}.html">Next: ${escapeHtml(next.title)}</a>` : "<span></span>"}
      </nav>
    </article>
  </main>
  ${pageScript}
</body>
</html>`;
}

function adminPortalScript() {
  return `<script>
(() => {
  const sourceToken = "__QPAYD_ADMIN_ASSET_SOURCE__";
  const integrityToken = "__QPAYD_ADMIN_ASSET_INTEGRITY__";
  const blocks = Array.from(document.querySelectorAll(".docs-article pre code"))
    .filter((block) => block.textContent.includes(sourceToken) || block.textContent.includes(integrityToken));

  if (blocks.length === 0) return;

  const status = document.createElement("p");
  status.className = "docs-release-status";
  status.textContent = "Loading latest release checksum...";
  blocks[0].closest("pre").before(status);

  fetch("./admin-portal.latest.json", { cache: "no-store" })
    .then((response) => {
      if (!response.ok) throw new Error("metadata request failed");
      return response.json();
    })
    .then((metadata) => {
      if (!metadata.asset_source || !metadata.asset_integrity || !metadata.qpayd_version) {
        throw new Error("metadata is incomplete");
      }

      for (const block of blocks) {
        block.textContent = block.textContent
          .replaceAll(sourceToken, metadata.asset_source)
          .replaceAll(integrityToken, metadata.asset_integrity);
      }

      status.textContent = \`Showing admin asset values for qpayd \${metadata.qpayd_version}.\`;
    })
    .catch(() => {
      status.textContent = "Latest release checksum metadata is unavailable. Check the release artifact admin-portal.json.";
    });
})();
</script>`;
}

async function siteCss() {
  const index = await fs.readFile(path.join(siteDir, "index.html"), "utf8");
  const match = index.match(/<style>([\s\S]*?)<\/style>/);
  if (!match) throw new Error("site/index.html has no style block");
  return `${match[1]}

    .docs-layout {
      display: grid;
      grid-template-columns: 260px minmax(0, 1fr);
      gap: 32px;
      align-items: start;
      padding: 42px 0 72px;
    }

    .docs-nav {
      position: sticky;
      top: 18px;
      display: grid;
      gap: 4px;
      padding: 12px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: var(--panel);
    }

    .docs-nav a {
      display: block;
      padding: 9px 10px;
      border-radius: 8px;
      color: var(--muted);
      font-size: 14px;
    }

    .docs-nav a:hover,
    .docs-nav a[aria-current="page"] {
      background: var(--panel-2);
      color: var(--text);
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

    .docs-article h3 {
      margin-top: 30px;
      font-size: 22px;
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

    .docs-article ul,
    .docs-article ol {
      padding-left: 22px;
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

    .docs-release-status {
      margin: 16px 0 10px;
      color: var(--muted) !important;
      font-size: 14px;
    }

    .anchor {
      margin-left: 8px;
      color: var(--muted) !important;
      text-decoration: none !important;
      opacity: .55;
    }

    .docs-pager {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 12px;
      margin-top: 44px;
      padding-top: 20px;
      border-top: 1px solid var(--line);
    }

    .docs-pager a {
      display: block;
      padding: 14px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: #0b1116;
      text-decoration: none;
    }

    @media (max-width: 900px) {
      .docs-layout {
        grid-template-columns: 1fr;
      }

      .docs-nav {
        position: static;
      }
    }

    @media (max-width: 620px) {
      .docs-article {
        padding: 22px;
      }

      .docs-pager {
        grid-template-columns: 1fr;
      }
    }
`;
}

function titleFromSlug(slug) {
  return slug
    .split("-")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
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
