// Sources, engine mappings and time windows — adapted from
// superagents-lab/jev-search (MIT). The laya judge answers the same
// yes/no question per source to decide whether it is wanted.

export const SOURCES = [
  {
    id: 'google', label: 'Google', lanes: [{ service: 'google' }], defaultOn: true,
    description: 'Google web results: news sites, blogs, documentation, anything on the open web',
    ask: {
      question: 'Would general web pages (news, articles, blogs, docs) help answer this request?',
      yes: 'The request is a general question, or asks for news, articles, coverage, docs or blog posts',
      no: 'The request only makes sense on a specific platform such as Reddit, GitHub, arXiv, YouTube or IMDb',
    },
  },
  {
    id: 'duckduckgo', label: 'DuckDuckGo', lanes: [{ service: 'duckduckgo' }], defaultOn: true,
    description: 'DuckDuckGo web results: a second, independent view of the open web',
    ask: {
      question: 'Would general web pages (news, articles, blogs, docs) help answer this request?',
      yes: 'The request is a general question, or asks for news, articles, coverage, docs or blog posts',
      no: 'The request only makes sense on a specific platform such as Reddit, GitHub, arXiv, YouTube or IMDb',
    },
  },
  {
    id: 'yandex', label: 'Yandex', lanes: [{ service: 'yandex' }], defaultOn: true,
    description: 'Yandex web results: a third, independent view of the open web, including Russian-language pages',
    ask: {
      question: 'Would general web pages (news, articles, blogs, docs) help answer this request?',
      yes: 'The request is a general question, or asks for news, articles, coverage, docs or blog posts',
      no: 'The request only makes sense on a specific platform such as Reddit, GitHub, arXiv, YouTube or IMDb',
    },
  },
  {
    id: 'hackernews', label: 'Hacker News', lanes: [{ service: 'google', site: 'news.ycombinator.com' }, { service: 'hackernews' }], defaultOn: false,
    description: 'Hacker News threads and comments',
    ask: {
      question: 'Would Hacker News threads fit this request?',
      yes: 'The request names Hacker News or HN, or asks what developers or the tech community are saying, their reactions, opinions or discussion about a technical topic',
      no: 'The request is a factual lookup, or is about something outside technology and startups',
    },
  },
  {
    id: 'reddit', label: 'Reddit', lanes: [{ service: 'google', site: 'reddit.com' }, { service: 'reddit' }], defaultOn: false,
    description: 'Reddit threads and comments',
    ask: {
      question: 'Would Reddit threads and comments fit this request?',
      yes: 'The request names Reddit, or asks what users or a community think, their reactions, opinions, recommendations or discussion about a product, game, place or topic',
      no: 'The request is a factual lookup, or needs code or academic papers rather than discussion',
    },
  },
  {
    id: 'github', label: 'GitHub', lanes: [{ service: 'google', site: 'github.com' }, { service: 'github' }], defaultOn: false,
    description: 'GitHub repositories, issues and releases',
    ask: {
      question: 'Is the user looking for code: repositories, releases, issues, pull requests or open source projects?',
      yes: 'The request names GitHub, or asks for repos, libraries, releases, issues, PRs, or open source tools',
      no: 'The request is about discussion, news or opinions rather than code',
    },
  },
  {
    id: 'x', label: 'X', lanes: [{ service: 'x' }], defaultOn: false,
    description: 'Posts on X (formerly Twitter)',
    ask: {
      question: 'Would posts on X (Twitter) fit this request?',
      yes: 'The request names X, Twitter or tweets, or asks what people are saying, their reactions, opinions or discussion about a product, launch, announcement, company or person, especially in tech and startups; launches and news break on X first',
      no: 'The request is a factual lookup, or asks for long-form content such as tutorials, papers or documentation',
    },
  },
  {
    id: 'arxiv', label: 'arXiv', lanes: [{ service: 'arxiv' }], defaultOn: false,
    description: 'Academic papers and preprints on arXiv',
    ask: {
      question: 'Is the user asking for academic papers, research or preprints?',
      yes: 'The request mentions papers, research, arXiv, studies or preprints',
      no: 'The request is not about academic research',
    },
  },
  {
    id: 'wikipedia', label: 'Wikipedia', lanes: [{ service: 'wikipedia', timeFilter: false }], defaultOn: false,
    description: 'Encyclopedia articles on Wikipedia',
    ask: {
      question: 'Is the user asking for encyclopedic facts, definitions, background or history?',
      yes: 'The request asks what or who something is, how it works, its history or background facts',
      no: 'The request asks for opinions, news, recent events, code, papers or videos',
    },
  },
  {
    id: 'imdb', label: 'IMDb', lanes: [{ service: 'imdb', timeFilter: false, entityQuery: true }], defaultOn: false,
    description: 'Movies, TV shows, actors and directors on IMDb',
    ask: {
      question: 'Is the user asking about a film, TV series, actor, director or other screen credit?',
      yes: 'The request names or describes a movie or show, or asks who acted in, directed or made one',
      no: 'The request is not about film or television',
    },
  },
  {
    id: 'wechat', label: 'WeChat', lanes: [{ service: 'wechat', timeFilter: false }], defaultOn: false,
    description: 'Articles from WeChat official accounts (微信公众号)',
    ask: {
      question: 'Would Chinese-language articles from WeChat official accounts (微信公众号) fit this request?',
      yes: 'The request mentions 微信, 公众号 or WeChat, or is written in Chinese and asks for articles, tutorials, analysis or opinions',
      no: 'The request is not in Chinese and does not mention WeChat',
    },
  },
  {
    id: 'youtube', label: 'YouTube', lanes: [{ service: 'youtube' }], defaultOn: false,
    description: 'Videos on YouTube',
    ask: {
      question: 'Is the user asking for videos?',
      yes: 'The request mentions videos, YouTube, talks, tutorials to watch, or channels',
      no: 'The request is not about video content',
    },
  },
];

export const SOURCE_IDS = SOURCES.map((s) => s.id);
export const DEFAULT_SOURCE_IDS = SOURCES.filter((s) => s.defaultOn).map((s) => s.id);

export function isSourceId(value) {
  return SOURCE_IDS.includes(value);
}

export function sourceById(id) {
  const found = SOURCES.find((s) => s.id === id);
  if (!found) throw new Error(`Unknown source: ${id}`);
  return found;
}

export const WINDOWS = [
  { id: 'any', label: 'Any time', hours: Number.POSITIVE_INFINITY, description: 'The request does not ask for recent results; older, evergreen pages are fine' },
  { id: '24h', label: 'Past 24 hours', hours: 24, timeRange: 'day', description: 'Only things from today or the last day' },
  { id: '7d', label: 'Past week', hours: 24 * 7, timeRange: 'week', description: 'Things from the last several days, up to a week' },
  { id: '30d', label: 'Past month', hours: 24 * 30, timeRange: 'month', description: 'Things from the last few weeks, up to a month' },
];

export const DEFAULT_WINDOW = 'any';

export function isWindowId(value) {
  return WINDOWS.some((w) => w.id === value);
}

export function windowById(id) {
  const found = WINDOWS.find((w) => w.id === id);
  if (!found) throw new Error(`Unknown window: ${id}`);
  return found;
}
