# Live-fire BDD corpus: 29 real websites, one workflow each

This is the *live-fire* half of the BDD work: 29 `.feature` workflows, each **>20 steps**,
each compiled from the closed step vocabulary and executed against a **real, external**
website over CDP with a headless Chrome. No fixture server, no mocks. It exists to answer
two questions the hermetic suite cannot: *does the vocabulary actually drive the real web*,
and *what does authoring a workflow in BDD cost versus writing the JSON spec by hand*.

- Corpus: `bdd/bench/live/*.feature` (+ a `<name>.config.json` sidecar naming the hosts it may reach).
- Runner: `laya-workflow bdd live` (live-fire mode: no fixtures, no probes, `base_url` empty so every URL is absolute).
- Result: **29 passed, 0 failed** — 842 steps, ~221 s serial on one headless Chrome.

## The corpus

| # | case | kind | URL | steps | verdict | sec |
|---|------|------|-----|------:|:-------:|----:|
| 01 | `example_domain` | minimal documentation page | <https://example.com/> | 30 | PASS | 2.9 |
| 02 | `wikipedia_rust` | encyclopedia article | <https://en.wikipedia.org/wiki/Rust_(programming_language)> | 29 | PASS | 5.1 |
| 03 | `wikipedia_systems` | encyclopedia article | <https://en.wikipedia.org/wiki/Systems_programming> | 29 | PASS | 3.0 |
| 04 | `wiktionary_serendipity` | dictionary entry | <https://en.wiktionary.org/wiki/serendipity> | 29 | PASS | 4.6 |
| 05 | `wiktionary_lexicon` | dictionary entry | <https://en.wiktionary.org/wiki/lexicon> | 29 | PASS | 2.8 |
| 06 | `hackernews` | news aggregator | <https://news.ycombinator.com/> | 29 | PASS | 3.4 |
| 07 | `github_rust` | code hosting | <https://github.com/rust-lang/rust> | 29 | PASS | 14.0 |
| 08 | `gitlab` | code hosting | <https://gitlab.com/gitlab-org/gitlab> | 29 | PASS | 13.6 |
| 09 | `pypi_requests` | package registry | <https://pypi.org/project/requests/> | 29 | PASS | 7.1 |
| 10 | `docsrs_serde` | API documentation | <https://docs.rs/serde/latest/serde/> | 29 | PASS | 6.6 |
| 11 | `mdn_html` | web-platform docs | <https://developer.mozilla.org/en-US/docs/Web/HTML> | 29 | PASS | 5.5 |
| 12 | `go_dev` | language home | <https://go.dev/> | 29 | PASS | 8.9 |
| 13 | `nodejs` | runtime home | <https://nodejs.org/en> | 29 | PASS | 4.7 |
| 14 | `arxiv_attention` | academic preprint | <https://arxiv.org/abs/1706.03762> | 29 | PASS | 4.7 |
| 15 | `kernel_org` | kernel project | <https://www.kernel.org/> | 29 | PASS | 3.8 |
| 16 | `debian` | OS distribution | <https://www.debian.org/> | 29 | PASS | 8.9 |
| 17 | `w3c` | standards body | <https://www.w3.org/> | 29 | PASS | 5.3 |
| 18 | `ietf` | standards body | <https://www.ietf.org/> | 29 | PASS | 4.4 |
| 19 | `rfc_editor` | standards document | <https://www.rfc-editor.org/info/rfc2119/> | 29 | PASS | 5.2 |
| 20 | `php_net` | language home | <https://www.php.net/> | 29 | PASS | 6.2 |
| 21 | `perl_org` | language home | <https://www.perl.org/> | 29 | PASS | 4.3 |
| 22 | `haskell_org` | language home | <https://www.haskell.org/> | 29 | PASS | 7.7 |
| 23 | `scala_lang` | language home | <https://www.scala-lang.org/> | 29 | PASS | 7.7 |
| 24 | `weather_gov` | government service | <https://www.weather.gov/> | 29 | PASS | 6.5 |
| 25 | `nasa_gov` | space agency | <https://www.nasa.gov/> | 29 | PASS | 37.3 |
| 26 | `npr_org` | news media | <https://www.npr.org/> | 29 | PASS | 20.4 |
| 27 | `gutenberg_pnp` | ebook library | <https://www.gutenberg.org/ebooks/1342> | 29 | PASS | 4.6 |
| 28 | `wikidata_q42` | structured knowledge | <https://www.wikidata.org/wiki/Q42> | 29 | PASS | 6.8 |
| 29 | `commons_wikimedia` | media repository | <https://commons.wikimedia.org/wiki/Main_Page> | 29 | PASS | 4.9 |

**Totals** — 29 cases, 842 steps, min 29 steps, max 30 steps, avg 29.0 steps/case; 221s of scenario time.

The corpus deliberately spans categories — an encyclopedia, two dictionaries, a news
aggregator, two code hosts, a package registry, API and web-platform docs, four language
homes, an academic preprint, a kernel project, an OS distribution, three standards sites,
a government weather service, a space agency, a news outlet, an ebook library, a structured
knowledge base and a media repository — because a step vocabulary that only survives one
CMS is not a vocabulary, it is a fixture.

## Efficiency: BDD vs hand-written JSON DSL

For each case the tool compiles the `.feature` and measures the JSON spec an author would
otherwise have to write by hand (`laya-workflow bdd compare`). Both are the *same workflow*;
the JSON is the compiled artifact, so the comparison is apples-to-apples.

### Authoring & delivery volume

| # | case | steps | .feature lines | .feature bytes | JSON lines | JSON bytes | JSON/BDD bytes |
|---|------|------:|---------------:|---------------:|-----------:|-----------:|---------------:|
| 01 | `example_domain` | 30 | 34 | 1766 | 1312 | 29866 | 16.9x |
| 02 | `wikipedia_rust` | 29 | 33 | 1935 | 1238 | 28892 | 14.9x |
| 03 | `wikipedia_systems` | 29 | 33 | 1877 | 1238 | 28718 | 15.3x |
| 04 | `wiktionary_serendipity` | 29 | 33 | 1850 | 1238 | 28638 | 15.5x |
| 05 | `wiktionary_lexicon` | 29 | 33 | 1846 | 1238 | 28626 | 15.5x |
| 06 | `hackernews` | 29 | 33 | 1825 | 1238 | 28566 | 15.7x |
| 07 | `github_rust` | 29 | 33 | 1795 | 1238 | 28466 | 15.9x |
| 08 | `gitlab` | 29 | 33 | 1786 | 1238 | 28439 | 15.9x |
| 09 | `pypi_requests` | 29 | 33 | 1784 | 1238 | 28431 | 15.9x |
| 10 | `docsrs_serde` | 29 | 33 | 1769 | 1238 | 28385 | 16.0x |
| 11 | `mdn_html` | 29 | 33 | 1816 | 1238 | 28540 | 15.7x |
| 12 | `go_dev` | 29 | 33 | 1689 | 1238 | 28144 | 16.7x |
| 13 | `nodejs` | 29 | 33 | 1738 | 1238 | 28295 | 16.3x |
| 14 | `arxiv_attention` | 29 | 33 | 1808 | 1238 | 28504 | 15.8x |
| 15 | `kernel_org` | 29 | 33 | 1763 | 1238 | 28374 | 16.1x |
| 16 | `debian` | 29 | 33 | 1762 | 1238 | 28371 | 16.1x |
| 17 | `w3c` | 29 | 33 | 1747 | 1238 | 28322 | 16.2x |
| 18 | `ietf` | 29 | 33 | 1736 | 1238 | 28291 | 16.3x |
| 19 | `rfc_editor` | 29 | 33 | 1803 | 1238 | 28498 | 15.8x |
| 20 | `php_net` | 29 | 33 | 1744 | 1238 | 28314 | 16.2x |
| 21 | `perl_org` | 29 | 33 | 1728 | 1238 | 28267 | 16.4x |
| 22 | `haskell_org` | 29 | 33 | 1795 | 1238 | 28471 | 15.9x |
| 23 | `scala_lang` | 29 | 33 | 1783 | 1238 | 28438 | 15.9x |
| 24 | `weather_gov` | 29 | 33 | 1820 | 1238 | 28546 | 15.7x |
| 25 | `nasa_gov` | 29 | 33 | 1750 | 1238 | 28333 | 16.2x |
| 26 | `npr_org` | 29 | 33 | 1743 | 1238 | 28311 | 16.2x |
| 27 | `gutenberg_pnp` | 29 | 33 | 1826 | 1238 | 28566 | 15.6x |
| 28 | `wikidata_q42` | 29 | 33 | 1784 | 1238 | 28439 | 15.9x |
| 29 | `commons_wikimedia` | 29 | 33 | 1890 | 1238 | 28762 | 15.2x |
| | **aggregate** | **842** | **958** | **51958** | **35976** | **826813** | **15.9x** |

The hand-written JSON DSL is **15.9x the bytes** and **37.6x the lines** of the identical workflow written as BDD. A ~29-step workflow is ~33 lines of readable Gherkin or ~1,238 lines of nested JSON nodes; the Gherkin is what a human reviews, the JSON is what the engine runs.

### The authoring gate (generation / debugging)

The decisive difference is *where a mistake is caught*:

| representation | gate an author runs | seeded fault | caught offline? |
|----------------|---------------------|--------------|:---------------:|
| BDD `.feature` | `bdd build` (compiler, closed vocabulary) | unknown step verb (`I tap the element …`) | **29/29** |
| JSON spec | `validate` (check_version + load_file + registry + secret audit) | unknown op (`"op":"frobnicate"`) | 0/87 |
| JSON spec | `validate` | edge to a missing node | 0/87 |
| JSON spec | `validate` | unknown assertion | 0/87 |
| JSON spec | `validate` (control) | removed `start` | **29/29** |

BDD is *vocabulary-closed*: a step that is not one of the ~30 known patterns is a hard
compile error, with a "did you mean" hint, before a browser ever opens. The JSON DSL has no
such vocabulary: its `validate` is structural only, so a typo'd op, a dangling edge target and
a misspelled assertion all pass `validate` and only fail when the workflow runs — against the
real site, minutes later. The control row proves this is a finding, not a broken probe: the
same `validate` catches a removed `start` immediately.

### Real debugging friction found on the live web

Three of the 29 cases failed on the first full run and were fixed. All three are the kind of
thing a hand-written JSON spec would hit too, but the BDD surface turns each into a one-line
edit:

1. **`example.com` has no anchors.** The template's `a[href]` steps failed there; the fix was
   a case-specific core using `<p>`. (A vocabulary without an escape hatch would have been
   stuck; `Then javascript "…" is true` and a per-case `.feature` are the escape hatch.)
2. **`www.rfc-editor.org/rfc/rfc2119` 302-redirects** to `/info/rfc2119/`. BDD's `open` and
   `navigate` wait for the requested URL to *match*, so a server redirect makes the step hang
   (`Chrome page did not finish loading … expectedMatch:false`). Fix: point the step at the
   canonical URL.
3. **`www.weather.gov/about/` client-side-redirects** (`location.href="https://www.weather.gov"`),
   so `navigate` observed a URL it never asked for (`target never reached … tries=100`). Fix:
   navigate to a page that stays put.

Two more authoring gotchas are worth recording because they are silent-failure shaped:

- `When I run javascript "x is true"` compiles to the **action** `evaluate` op, which runs the
  string as JavaScript — `"x is true"` is a `SyntaxError`. Use `Then javascript "x" is true`
  (the assert family). Two nearly identical sentences, opposite meanings: exactly the
  ambiguity a closed vocabulary is supposed to make explicit.
- A selector containing `"` (e.g. `a[href^="http"]`) must have the inner quotes escaped
  (`\"`) inside the Gherkin string.

## Reproduce

```sh
# 1. offline: compile the whole live corpus; measure volume + the authoring gate
laya-workflow bdd compare --json bdd/bench/live/compare.json

# 2. live: run all 29 cases against the real sites (needs Chrome)
CHROME_BIN=/usr/bin/chromium-browser laya-workflow bdd live \
    --json bdd/bench/live/live_report.json

# one case, or a subset
CHROME_BIN=/usr/bin/chromium-browser laya-workflow bdd run \
    bdd/bench/live/02_wikipedia_rust.feature --live
```

## Verdict

The step vocabulary is real: the same ~30 patterns drove 29 unrelated websites — from a
576-byte static page to a 37-second JavaScript application — with no site-specific engine
support, only site-specific *selectors* inside otherwise identical Gherkin. And the authoring
surface is decisively cheaper: **~16x fewer bytes, ~38x fewer lines, and all 29 seeded faults
caught offline at compile time instead of on a live page at run time** compared to hand-written
JSON DSL.
