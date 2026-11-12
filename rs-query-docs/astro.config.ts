import { defineDocsConfig } from '@elcoosp/docs-toolkit/config';
import linksValidator from 'starlight-links-validator';
import imageZoom from 'starlight-image-zoom';
import headingBadges from 'starlight-heading-badges';

export default await defineDocsConfig({
  llmsTxt: {},
  changelog: { repo: 'elcoosp/rs-query' },
  accentOverride: '#3e3e74',
  site: 'https://rs-query.vercel.app',
  domain: 'rs-query.vercel.app',
  title: "rs-query",
  description: "TanStack Query v5 parity for Rust — declarative, cache-aware async state management portable to every quoin-supported UI framework.",
  sidebar: [],
  extraStarlight: {
                        expressiveCode: {
                        	themes: ['github-light', 'github-dark']
                        },
                        title: 'rs-query',
                        description:
                                'TanStack Query v5 parity for Rust — declarative, cache-aware async state management portable to every quoin-supported UI framework.',
                        favicon: '/favicon.png',
                        social: [
                                {
                                        icon: 'github',
                                        label: 'GitHub',
                                        href: 'https://github.com/elcoosp/rs-query'
                                },
                        ],
                        editLink: {
                                baseUrl: 'https://github.com/elcoosp/rs-query/edit/main/docs-site/src/content/docs/'
                        },

                        // The 404 page is a normal docs entry (src/content/docs/404.mdx) rendered
                        // through [...slug]; disabling Starlight's injected /404 route avoids the
                        // route-conflict warning between the two handlers.
                        disable404Route: true,

                        plugins: [linksValidator(), imageZoom(), headingBadges()],
                        sidebar: [
                                {
                                        label: 'Tutorials', translations: { fr: 'Tutoriels' },
                                        items: ['tutorials/first-query', 'tutorials/quoin-app']
                                },
                                {
                                        label: 'Guides', translations: { fr: 'Guides' },
                                        items: [
                                                'guides/install',
                                                'guides/queries',
                                                'guides/keys-and-filters',
                                                'guides/mutations',
                                                'guides/infinite-queries',
                                                'guides/streamed-queries',
                                                'guides/offline-and-network-modes',
                                                'guides/persistence',
                                                'guides/hydration',
                                                'guides/framework-bindings',
                                                'guides/devtools',
                                        ]
                                },
                                {
                                        label: 'Reference', translations: { fr: 'Référence' },
                                        items: [
                                                {
                                                        label: 'Core API',
                                                        items: [
                                                                'reference/client',
                                                                'reference/query',
                                                                'reference/query-options',
                                                                'reference/keys-and-filters',
                                                                'reference/mutations',
                                                                'reference/infinite-queries',
                                                                'reference/observers',
                                                                'reference/hooks',
                                                        ]
                                                },
                                                {
                                                        label: 'Runtime & Ecosystem',
                                                        items: [
                                                                'reference/runtime-and-network',
                                                                'reference/persistence-and-hydration',
                                                                'reference/errors-and-cancellation',
                                                                'reference/utilities',
                                                        ]
                                                },
                                        ]
                                },
                                {
                                        label: 'Explanation', translations: { fr: 'Explication' },
                                        items: [
                                                'explanation/architecture',
                                                'explanation/caching-model',
                                                'explanation/tanstack-parity',
                                        ]
                                },
                        ]
                },
});
