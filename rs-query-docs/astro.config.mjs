import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import linksValidator from 'starlight-links-validator';
import imageZoom from 'starlight-image-zoom';
import headingBadges from 'starlight-heading-badges';

export default defineConfig({
        // GitHub Pages project-site deployment for the rs-query repository (see
        // docs/decisions/0006-site-and-deployment.md). `site` is also required by
        // Starlight's built-in sitemap integration, which warns without it — the
        // zero-warning release gate leaves no opt-out.
        site: 'https://elcoosp.github.io',
        base: '/rs-query',
        integrations: [
                starlight({
                        title: 'rs-query',
                        description:
                                'TanStack Query v5 parity for Rust — declarative, cache-aware async state management portable to every quoin-supported UI framework.',
                        favicon: '/favicon.png',
                        logo: {
                                light: './src/assets/logo-light.png',
                                dark: './src/assets/logo-dark.png',
                        },
                        customCss: ['./src/styles/custom.css'],
                        social: [
                                {
                                        icon: 'github',
                                        label: 'GitHub',
                                        href: 'https://github.com/elcoosp/rs-query',
                                },
                        ],
                        editLink: {
                                baseUrl: 'https://github.com/elcoosp/rs-query/edit/main/docs-site/src/content/docs/',
                        },
                        defaultLocale: 'root',
                        // The 404 page is a normal docs entry (src/content/docs/404.mdx) rendered
                        // through [...slug]; disabling Starlight's injected /404 route avoids the
                        // route-conflict warning between the two handlers.
                        disable404Route: true,
                        locales: {
                                root: {
                                        label: 'English',
                                        lang: 'en',
                                },
                        },
                        plugins: [linksValidator(), imageZoom(), headingBadges()],
                        sidebar: [
                                {
                                        label: 'Tutorials',
                                        items: ['tutorials/first-query', 'tutorials/quoin-app'],
                                },
                                {
                                        label: 'Guides',
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
                                        ],
                                },
                                {
                                        label: 'Reference',
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
                                                        ],
                                                },
                                                {
                                                        label: 'Runtime & Ecosystem',
                                                        items: [
                                                                'reference/runtime-and-network',
                                                                'reference/persistence-and-hydration',
                                                                'reference/errors-and-cancellation',
                                                                'reference/utilities',
                                                        ],
                                                },
                                        ],
                                },
                                {
                                        label: 'Explanation',
                                        items: [
                                                'explanation/architecture',
                                                'explanation/caching-model',
                                                'explanation/tanstack-parity',
                                        ],
                                },
                        ],
                }),
        ],
});
