// The Bridle documentation site (Astro Starlight). The pages are built from ../docs by scripts/prepare-docs.mjs.
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { BASE, SITE_URL } from './site.config.mjs';

export default defineConfig({
  site: SITE_URL,
  base: BASE,
  integrations: [
    starlight({
      title: 'Bridle',
      description: 'A local-first coding agent for Windows. It runs on your own GPU, and every action it takes passes through a policy you can read.',
      favicon: '/favicon.svg',
      customCss: ['./src/styles/custom.css'],
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/kushagrathisside/bridle' }],
      sidebar: [
        {
          label: 'Start',
          items: [
            'introduction',
            'getting-started',
            'examples',
            'examples/fix-a-failing-test',
          ],
        },
        {
          label: 'Guides',
          items: [
            'guides/install',
            'guides/set-up-a-repository',
            'guides/run-verification',
            'guides/serve-models',
            'guides/models-runtimes-and-plugins',
            'guides/run-a-task',
            'guides/troubleshoot',
          ],
        },
        {
          label: 'Using Bridle',
          items: [
            'guides/desktop',
            'guides/tui',
            'guides/team-and-board',
            'guides/lanes',
            'guides/fleet',
            'guides/background-commands',
            'guides/web-research',
            'guides/web-browser',
            'guides/window-screenshots',
            'guides/windows-only',
          ],
        },
        {
          label: 'Concepts',
          items: [
            'concepts/how-decisions-are-made',
            'concepts/tasks-and-loops',
            'concepts/models-and-slots',
          ],
        },
        {
          label: 'Reference',
          items: [
            'reference/cli',
            'reference/configuration',
            'reference/mcp-tools',
            'reference/run-recovery',
            { label: 'Developer modules', items: [{ autogenerate: { directory: 'reference/developer' } }] },
          ],
        },
        {
          label: 'Project',
          items: [
            'project/principles',
            'project/architecture',
            'project/security-model',
            'project/status',
            'project/contributing',
            'project/releasing',
            'project/lessons-from-infinity-and-harness2',
            'project/graph-engineering-plan',
            'project/changelog',
            'project/security-policy',
            { label: 'Decisions', items: [{ autogenerate: { directory: 'project/decisions' } }] },
          ],
        },
      ],
    }),
  ],
});
