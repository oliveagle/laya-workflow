// Search1API client — adapted from superagents-lab/jev-search (MIT).

const ENTITIES = {
    amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', middot: '·', hellip: '…',
    mdash: '—', ndash: '–', lsquo: '‘', rsquo: '’', ldquo: '“', rdquo: '”', laquo: '«', raquo: '»',
};
/** Snippets come back with HTML entities left in; decode the common ones. */
export function decodeEntities(text) {
    return text
        .replace(/&#x([0-9a-f]+);/gi, (_, hex) => String.fromCodePoint(Number.parseInt(hex, 16)))
        .replace(/&#(\d+);/g, (_, dec) => String.fromCodePoint(Number(dec)))
        .replace(/&([a-z]+);/gi, (match, name) => ENTITIES[name.toLowerCase()] ?? match)
        .replace(/\s{2,}/g, ' ');
}
export class Search1ApiError extends Error {
    status;
    constructor(status, message) {
        super(message);
        this.name = 'Search1ApiError';
        this.status = status;
    }
}
/**
 * One POST /search, or /news for Hacker News. Source restriction is done with
 * `include_sites` / `exclude_sites`; vertical engines are picked with
 * `service`. Recency is `time_range` in both cases.
 */
/** Allow filtered searches to finish while other lanes stream independently. */
export const LANE_TIMEOUT_MS = 15_000;
export async function search(config, params, signal) {
    const base = (config.baseUrl ?? 'https://api.search1api.com').replace(/\/$/, '');
    const timeout = AbortSignal.timeout(LANE_TIMEOUT_MS);
    const laneSignal = signal ? AbortSignal.any([signal, timeout]) : timeout;
    const response = await fetch(`${base}/${params.service === 'hackernews' ? 'news' : 'search'}`, {
        method: 'POST',
        headers: {
            Authorization: `Bearer ${config.apiKey}`,
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({
            query: params.query,
            search_service: params.service ?? 'google',
            ...(params.timeRange ? { time_range: params.timeRange } : {}),
            max_results: params.maxResults ?? 8,
            include_sites: params.includeSites ?? [],
            exclude_sites: params.excludeSites ?? [],
        }),
        signal: laneSignal,
    });
    if (!response.ok) {
        const text = await response.text().catch(() => '');
        throw new Search1ApiError(response.status, text.slice(0, 300) || response.statusText);
    }
    const body = (await response.json());
    const results = Array.isArray(body.results) ? body.results : [];
    return results
        .filter((r) => typeof r === 'object' &&
        r !== null &&
        typeof r.link === 'string' &&
        typeof r.title === 'string')
        .map((r) => ({
        title: decodeEntities(r.title),
        link: r.link,
        snippet: decodeEntities(r.snippet ?? ''),
        ...(typeof r.published_date === 'string' ? { published_date: r.published_date } : {}),
    }));
}
