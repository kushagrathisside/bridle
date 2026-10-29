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
      description: 'A local-first harness and orchestrator for multi-agent coding.',
      favicon: '/favicon.svg',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/kushagrathisside/bridle' }],
      sidebar: [
        { label: 'Concepts', items: [{ autogenerate: { directory: 'concepts' } }] },
        { label: 'Guide', items: [{ autogenerate: { directory: 'guide' } }] },
        { label: 'Decisions', items: [{ autogenerate: { directory: 'decisions' } }] },
        { label: 'Reference', items: [{ autogenerate: { directory: 'reference' } }] },
      ],
    }),
  ],
});
