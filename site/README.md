# The Bridle site

An Astro site with two parts: a **home page** written for this site, and a **documentation site** (Starlight) that is generated from
the Markdown in this repository. Deployed to GitHub Pages by `.github/workflows/site.yml`.

```bat
cd site
npm ci
npm run build      :: prepare-docs.mjs, then astro build (pages, sitemap, search index)
npm run preview    :: serve the build at http://127.0.0.1:4321/bridle/
```

## Layout

| Path | What it is |
|---|---|
| `src/pages/index.astro` | The home page. It replaces Starlight's own home route; every documentation route is unchanged. |
| `src/layouts/Home.astro` | The page shell: metadata, the theme script, the navigation, the search dialog and the footer. |
| `src/components/home/` | One component per section: `Hero`, `Why`, `Tour`, `Architecture`, `PolicyLab`, `Capabilities`, `Security`, `GetStarted`, `DocsIndex`, `Closing`. |
| `src/scripts/` | The small client scripts (TypeScript). Each section loads only its own. Search loads Pagefind the first time it opens. |
| `src/styles/tokens.css` | Every colour, size and font, for both themes. The documentation uses the same tokens (`custom.css`). |
| `src/styles/home/` | The home page's styles, split by area. |
| `src/data/` | The content the sections are built from: `architecture.ts`, `tour.ts`, `lab.ts`, `policy.ts` and the recorded answers in `policy-recorded.json`. |
| `src/assets/shots/` | Screenshots of the real window, used by the walkthrough and the hero. |
| `public/media/` | The recording shown in the hero. |
| `scripts/prepare-docs.mjs` | Writes the documentation pages from `docs/` and the root Markdown files. Nothing in `src/content/docs/` is edited by hand. |

## What is recorded, and how to record it again

The home page never evaluates a command. Everything that looks like a policy decision is the recorded output of the real program.

- **Policy answers** (`src/data/policy-recorded.json`). Run the built program through `scripts/record-policy.mjs`:

  ```bat
  node scripts/record-policy.mjs --bridle D:\cargo-target\harness\release\bridle.exe
  ```

  It sets up a throwaway project with `bridle init`, sends each example to `bridle hook tool-before`, reproduces the loop refusal, and
  records what `tool-after` redacts. It uses a temporary `APPDATA`, so your own configuration is not touched. Review the diff of the JSON
  file after a policy change; the build fails if a case the page needs is missing.
- **Screenshots and the recording.** They are of the shipped `bridle.exe` on a clone of [more-itertools](https://github.com/more-itertools/more-itertools)
  (MIT), driven through the window's debug port with the bounded client in `tools/e2e/cdp.py`. The team is shown under its role names. The
  recording is a screencast of the real window with waiting cut and typing sped up.
- **The version** on the page is read from `[workspace.package]` in `Cargo.toml` at build time.

## Checks

```bat
node scripts/check-contrast.mjs   :: WCAG contrast of every text colour on its surface, both themes
```

The build fails on a broken link in the generated documentation. Before a release, also check the home page at 1440, 1280, 1024, 768,
390 and 360 px, in both themes, with the keyboard, and with reduced motion switched on.
