@decision
Feature: google_search_open_top10
  Port of google_search_top10: converted from dsl/browser/google_search_top10.json (BDD round-trip).

  Background:
    Given the workflow runs at most 16 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | max_timeout_ms | 30000 |
      | max_output | 1048576 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":15000,"max_text":8000} |

  Scenario: open_google
    Given a choice question "proceed" asks "Open Google in the singleton browser?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "wait_home"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | https://www.google.com/ |
      | project.target_id | /target_id |

  Scenario: wait_home
    Given a choice question "proceed" asks "Continue after the Google search box is visible?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "type_query"
    Then call the capability "chrome"
      | with.op | wait_for |
      | with.target_id | ${state.target_id} |
      | with.selector | textarea[name=q], input[name=q] |
      | with.timeout_ms | 15000 |
      | project.home_ready | /ready |

  Scenario: type_query
    Given a choice question "proceed" asks "Type the requested search query?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "submit_search"
    Then call the capability "chrome"
      | with.op | type |
      | with.target_id | ${state.target_id} |
      | with.selector | textarea[name=q], input[name=q] |
      | with.text | ${state.query} |
      | project.typed | /typed |

  Scenario: submit_search
    Given a choice question "proceed" asks "Open Google's result page for the query?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "wait_results"
    Then call the capability "chrome"
      | with.op | navigate |
      | with.target_id | ${state.target_id} |
      | with.url | https://www.google.com/search?q=${state.query}&num=20&filter=0&hl=zh-CN |
      | with.timeout_ms | 20000 |
      | project.submitted | /ready |

  Scenario: wait_results
    Given a choice question "proceed" asks "Continue after Google results become visible?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "extract_page1"
    Then call the capability "chrome"
      | with.op | wait_for |
      | with.target_id | ${state.target_id} |
      | with.selector | #search, #rso |
      | with.timeout_ms | 15000 |
      | project.results_ready | /ready |

  Scenario: extract_page1
    Given a choice question "proceed" asks "Extract the first result page?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "navigate_page2"
    Then call the capability "chrome"
      | with.op | evaluate |
      | with.target_id | ${state.target_id} |
      | with.expression | "(() => { const seen = new Set(); const links = []; for (const h3 of document.querySelectorAll('#search a h3, #rso a h3')) { const a = h3.closest('a'); if (!a?.href \u007c\u007c !/^https?:/i.test(a.href)) continue; const title = (h3.innerText \u007c\u007c h3.textContent \u007c\u007c '').trim(); if (!title \u007c\u007c seen.has(title)) continue; seen.add(title); links.push({title, url: a.href}); } return {count: links.length, results: links, urls: links.map(x => x.url)}; })()" |
      | project.page1_count | /value/count |
      | project.page1_results | /value/results |
      | project.urls | /value/urls |

  Scenario: navigate_page2
    Given a choice question "proceed" asks "Fetch the next result page when needed to complete ten entries?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "extract_page2"
    Then call the capability "chrome"
      | with.op | navigate |
      | with.target_id | ${state.target_id} |
      | with.url | https://www.google.com/search?q=${state.query}&num=20&filter=0&hl=zh-CN&start=8 |
      | with.timeout_ms | 20000 |
      | project.page2_submitted | /ready |

  Scenario: extract_page2
    Given a choice question "proceed" asks "Merge the next result page into the first ten?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "open_top10"
    Then call the capability "chrome"
      | with.op | evaluate |
      | with.target_id | ${state.target_id} |
      | with.expression | "(() => { const previous = ${state.page1_results}; const seen = new Set(previous.map(x => x.title)); const links = [...previous]; for (const h3 of document.querySelectorAll('#search a h3, #rso a h3')) { const a = h3.closest('a'); if (!a?.href \u007c\u007c !/^https?:/i.test(a.href)) continue; const title = (h3.innerText \u007c\u007c h3.textContent \u007c\u007c '').trim(); if (!title \u007c\u007c seen.has(title)) continue; seen.add(title); links.push({title, url: a.href}); if (links.length >= 10) break; } return {count: links.length, requested_count: 10, results: links.slice(0, 10), urls: links.slice(0, 10).map(x => x.url)}; })()" |
      | project.result_count | /value/count |
      | project.results | /value/results |
      | project.urls | /value/urls |

  Scenario: open_top10
    Given a choice question "done" asks "Open all ten search results?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "chrome"
      | with.op | open_many |
      | with.urls | ${state.urls} |
      | with.keep_open | true |
      | with.tab_concurrency | ${state.tab_concurrency} |
      | with.skip_video | ${state.skip_video} |
      | with.skip_kinds | ${state.skip_kinds} |
      | project.opened | /opened |
      | project.opened_count | /opened_count |
      | project.skipped_count | /skipped_count |
      | project.skipped | /skipped |
