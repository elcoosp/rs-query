// Generates public/llms.txt and public/llms-full.txt from the docs content
// so both human readers and AI agents can discover the site.
//
// - llms.txt      a curated index: one section per Diátaxis type.
// - llms-full.txt the full body text of every page (frontmatter and MDX
//                 imports stripped), in sidebar order.
//
// Runs as part of `pnpm build` (see package.json).
import { readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

const DOCS_DIR = 'src/content/docs';
const SITE_TITLE = 'rs-query';
const SITE_DESCRIPTION =
	'TanStack Query v5 parity for Rust — declarative, cache-aware async state management portable to every quoin-supported UI framework.';
const REPO = 'https://github.com/elcoosp/rs-query';

async function walk(dir) {
	const entries = await readdir(dir, { withFileTypes: true });
	const files = [];
	for (const e of entries) {
		const p = path.join(dir, e.name);
		if (e.isDirectory()) files.push(...(await walk(p)));
		else if (/\.(md|mdx)$/.test(e.name)) files.push(p);
	}
	return files;
}

function parseFrontmatter(src) {
	const m = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(src);
	if (!m) return { fm: {}, body: src };
	const fm = {};
	for (const line of m[1].split(/\r?\n/)) {
		const kv = /^(\w[\w-]*):\s*"?(.*?)"?\s*$/.exec(line);
		if (kv) fm[kv[1]] = kv[2];
	}
	return { fm, body: src.slice(m[0].length) };
}

/** Strip MDX import lines and blank-runs so agents get clean prose + code. */
function cleanBody(body) {
	return body
		.split('\n')
		.filter((l) => !/^import\s+.*from\s+['"]/.test(l) && !/^export\s+(const|function|default)\s/.test(l))
		.join('\n')
		.replace(/\n{3,}/g, '\n\n')
		.trim();
}

/** File path → Starlight URL path (Starlight slug = path minus extension). */
function toUrl(rel) {
	const noExt = rel.replace(/\.(md|mdx)$/, '');
	if (noExt === 'index') return '/';
	return '/' + noExt + '/';
}

const TYPE_ORDER = ['overview', 'tutorial', 'how-to', 'reference', 'explanation'];
const TYPE_LABEL = {
	overview: 'Overview',
	tutorial: 'Tutorials — learning-oriented',
	'how-to': 'Guides — task-oriented how-tos',
	reference: 'Reference — information-oriented API docs',
	explanation: 'Explanation — understanding-oriented',
};

const files = (await walk(DOCS_DIR)).sort();
const pages = [];
for (const file of files) {
	const src = await readFile(file, 'utf8');
	const { fm, body } = parseFrontmatter(src);
	const rel = path.relative(DOCS_DIR, file).replace(/\\/g, '/');
	pages.push({
		file: rel,
		url: toUrl(rel),
		title: fm.title ?? rel,
		description: fm.description ?? '',
		type: fm.type ?? 'explanation',
		body: cleanBody(body),
	});
}

// ---- llms.txt -------------------------------------------------------------
let out = `# ${SITE_TITLE}\n\n> ${SITE_DESCRIPTION}\n\n${REPO}\n\n`;
for (const type of TYPE_ORDER) {
	const group = pages.filter((p) => p.type === type);
	if (group.length === 0) continue;
	out += `## ${TYPE_LABEL[type]}\n\n`;
	for (const p of group) {
		out += `- [${p.title}](${p.url}): ${p.description}\n`;
	}
	out += '\n';
}
out += '## Repository\n\n';
out += `- [Source code](${REPO}): the rs-query crate this documentation describes.\n`;
await writeFile('public/llms.txt', out);

// ---- llms-full.txt --------------------------------------------------------
let full = `# ${SITE_TITLE} — full documentation\n\n> ${SITE_DESCRIPTION}\n\n`;
for (const p of pages) {
	full += `\n\n---\n\n# ${p.title}\n\nURL: ${p.url}\nType: ${p.type}\n\n${p.body}\n`;
}
await writeFile('public/llms-full.txt', full);

console.log(`llms.txt: ${pages.length} pages indexed; llms-full.txt: ${full.length} chars`);
