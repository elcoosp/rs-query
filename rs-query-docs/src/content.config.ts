import { defineCollection } from 'astro:content';
import { docsLoader, i18nLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { z } from 'astro/zod';

const docs = defineCollection({
        loader: docsLoader(),
        schema: docsSchema({
                // A standalone zod object (not derived from the callback context):
                // Astro 7.4's JSON-schema generation calls the schema factory without a
                // `schema` property in the context, so context-based extends crash.
                // Starlight deep-merges this object into its frontmatter schema, which
                // keeps `type` required on every docs page.
                extend: z.object({
                        /** Diátaxis information type. `overview` is reserved for the landing page hub. */
                        type: z.enum(['tutorial', 'how-to', 'reference', 'explanation', 'overview']),
                }),
        }),
});

const i18n = defineCollection({ loader: i18nLoader() });

export const collections = { docs, i18n };
