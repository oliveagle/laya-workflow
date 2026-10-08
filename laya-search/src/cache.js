// Per-engine response cache with an in-memory implementation — adapted from
// superagents-lab/jev-search (MIT).

/** How long a lane's rows stay fresh, by how recent the user wants them. */
const TTL_SECONDS = {
    day: 10 * 60,
    week: 60 * 60,
    month: 2 * 60 * 60,
    any: 6 * 60 * 60,
};
export function cacheKey(params) {
    return [
        'v2', // v1 discarded published_date before caching results.
        params.service ?? 'google',
        params.query.trim().toLowerCase(),
        params.timeRange ?? 'any',
        (params.includeSites ?? []).join(','),
        (params.excludeSites ?? []).join(','),
        params.maxResults ?? 8,
    ].join('|');
}
export function cacheTtl(params) {
    return TTL_SECONDS[params.timeRange ?? 'any'] ?? TTL_SECONDS.any;
}
export async function cachedSearch(cache, params, run) {
    if (!cache)
        return { results: await run(), cached: false };
    const key = cacheKey(params);
    try {
        const hit = await cache.get(key);
        if (hit)
            return { results: JSON.parse(hit), cached: true };
    }
    catch {
        // A broken cache must never break a search.
    }
    const results = await run();
    if (results.length > 0) {
        await cache.put(key, JSON.stringify(results), { expirationTtl: cacheTtl(params) }).catch(() => undefined);
    }
    return { results, cached: false };
}
/** In-memory cache for tests and scripts. */
export function memoryCache() {
    const map = new Map();
    return {
        get: async (k) => map.get(k) ?? null,
        put: async (k, v) => {
            map.set(k, v);
        },
        get size() {
            return map.size;
        },
    };
}
