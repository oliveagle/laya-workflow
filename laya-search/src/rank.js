// Ranking, canonical URLs and near-duplicate clustering — adapted from
// superagents-lab/jev-search (MIT).

/**
 * Ordering is exactly what the row shows. Best match: the judge's on-topic
 * percentage, ties broken by how many engines agreed, then engine rank.
 * Newest: known age first, ties by on-topic; unknown age goes last.
 */
export function compareItems(a, b, mode) {
    if (a.ranked !== b.ranked)
        return a.ranked ? -1 : 1; // unranked rows wait at the bottom
    if (mode === 'newest') {
        const aa = a.ageHours ?? Number.POSITIVE_INFINITY;
        const bb = b.ageHours ?? Number.POSITIVE_INFINITY;
        if (aa !== bb)
            return aa - bb;
    }
    const ra = Math.round(a.relevance * 100);
    const rb = Math.round(b.relevance * 100);
    if (ra !== rb)
        return rb - ra;
    if (a.engines.length !== b.engines.length)
        return b.engines.length - a.engines.length;
    return a.position - b.position;
}
const TITLE_NOISE = [
    /\s*[-|–—:]\s*(reddit|hacker news|github|x)\s*$/i,
    /\s*:\s*r\/[\w]+\s*(-\s*reddit)?\s*$/i,
    /\s*·\s*github\s*$/i,
    /\s*\/\s*x\s*$/i,
    /\s*on x:\s*/i,
];
export function titleKey(title) {
    let t = title.toLowerCase();
    for (const re of TITLE_NOISE)
        t = t.replace(re, '');
    return t
        .replace(/[^\p{L}\p{N}]+/gu, ' ')
        .trim()
        .split(' ')
        .slice(0, 8)
        .join(' ');
}
const TRACKING_PARAM = /^(utm_|ref$|ref_|fbclid|gclid|igshid|share_id|rdt|si$|feature$|lang$|s$|t$)/i;
/** Host + path + the query params that identify content (YouTube's `v`), minus tracking noise. */
export function canonicalUrl(url) {
    try {
        const u = new URL(url);
        let host = u.hostname.toLowerCase().replace(/^www\./, '');
        if (host === 'twitter.com')
            host = 'x.com';
        const path = u.pathname.replace(/\/+$/, '').toLowerCase();
        const params = [...u.searchParams.entries()]
            .filter(([k]) => !TRACKING_PARAM.test(k))
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([k, v]) => `${k}=${v}`)
            .join('&');
        return `${host}${path}${params ? `?${params}` : ''}`;
    }
    catch {
        return url;
    }
}
/**
 * Group near-duplicates (same URL or same normalized title) so one story does
 * not occupy five slots. Ordering is by the lead's composite score.
 */
export function clusterItems(items, mode = 'best') {
    return clusterInOrder([...items].sort((a, b) => compareItems(a, b, mode)));
}
/**
 * Same grouping, but the lead order is the order given. Used while results
 * stream in so rows the reader has already seen do not move.
 */
export function clusterInOrder(items) {
    const byUrl = new Map();
    const byTitle = new Map();
    const clusters = [];
    for (const item of items) {
        const urlKey = canonicalUrl(item.url);
        const tKey = titleKey(item.title);
        const existing = byUrl.get(urlKey) ?? (tKey ? byTitle.get(tKey) : undefined);
        if (existing) {
            existing.others.push(item);
            continue;
        }
        const cluster = { lead: item, others: [] };
        clusters.push(cluster);
        byUrl.set(urlKey, cluster);
        if (tKey)
            byTitle.set(tKey, cluster);
    }
    return clusters;
}
