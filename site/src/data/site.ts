// Facts and links the home page shares. The version comes from the workspace manifest at build time, so the page cannot
// say a version the code does not have.
import fs from 'node:fs';
import path from 'node:path';
import { BASE, SITE_URL } from '../../site.config.mjs';

function workspaceVersion(): string {
  // The site is always built from site/, one level below the repository root.
  const manifest = fs.readFileSync(path.resolve(process.cwd(), '..', 'Cargo.toml'), 'utf8');
  const section = manifest.slice(manifest.indexOf('[workspace.package]'));
  const found = section.match(/^version\s*=\s*"([^"]+)"/m);
  if (!found) throw new Error('site: no version under [workspace.package] in Cargo.toml');
  return found[1];
}

export const version = workspaceVersion();
export const REPO = 'https://github.com/kushagrathisside/bridle';
export const base = BASE;
export const origin = SITE_URL;
export const homeUrl = `${SITE_URL}${BASE}/`;

export const zipName = `bridle-${version}-windows-x64.zip`;

export const links = {
  repo: REPO,
  releases: `${REPO}/releases/latest`,
  releaseNotes: `${REPO}/releases/tag/v${version}`,
  issues: `${REPO}/issues`,
  license: `${REPO}/blob/main/LICENSE`,
  source: (file: string) => `${REPO}/blob/main/${file}`,
  tree: (dir: string) => `${REPO}/tree/main/${dir}`,
};

/** A documentation page on this site, by its route ("guides/install"). */
export const doc = (route: string, hash = ''): string => `${BASE}/${route}/${hash}`;

export const title = 'Bridle: the model proposes, Bridle decides';
export const description =
  'Bridle is a local-first coding agent for Windows. Models run on your own GPU, and a policy written in code judges every tool call before it runs.';
