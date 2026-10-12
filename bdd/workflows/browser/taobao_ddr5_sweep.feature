@decision
Feature: taobao_ddr5_sweep
  Port of taobao_ddr5_sweep: converted from dsl/browser/taobao_ddr5_sweep.json (BDD round-trip).

  Background:
    Given the workflow runs at most 126 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | max_timeout_ms | 30000 |
      | max_output | 2097152 |
      | retries | 0 |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/jev-deep" |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":25000,"max_text":12000} |
      | planner | {"kind":"plugin","dir":"plugins/jev-planner","browser":"chrome","max_operations":8000000,"timeout_ms":25000} |
      | journal | {"kind":"file","op":"append","max_bytes":524288} |

  Scenario: open
    Given a choice question "go" asks "Open ONE owned tab in the shared singleton browser and keep it open at the end (keep_open), so the final product page stays on screen?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "home"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | about:blank |
      | with.keep_open | true |
      | project.browser_target | /target_id |
      | project.browser_kept_open | /keep_open |

  Scenario: home
    Given a choice question "go" asks "Navigate the owned tab to the Taobao home page and wait for the real commit?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "dismiss"
    Then call the capability "planner"
      | with.op | visit |
      | with.target_id | ${state.browser_target} |
      | with.url | https://www.taobao.com/ |
      | project.landing_url | /url |
      | project.landing_title | /title |

  Scenario: guard_home
    Given a choice question "go" asks "Wait out an anti-bot wall (滑块/验证码/访问太频繁/惩罚页) that the dismiss could not close?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "type_query"
    Then call the capability "planner"
      | with.op | guard |
      | with.target_id | ${state.browser_target} |
      | with.url | https://www.taobao.com/ |
      | with.wait_ms | 90000 |
      | project.wall | /wall |
      | project.guard_attempts | /attempts |
      | project.guard_waited_ms | /waited_ms |

  Scenario: dismiss
    Given a choice question "go" asks "Close the anti-bot challenge box / a full-viewport promo or login modal so later clicks land?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "guard_home"
    Then call the capability "planner"
      | with.op | dismiss |
      | with.target_id | ${state.browser_target} |
      | project.dismiss_overlays | /overlays |
      | project.dismiss_closed | /closed |
      | project.dismiss_blocking | /still_blocking |

  Scenario: type_query
    Given a choice question "go" asks "Fill the search box (a choice box) by number, with real CDP input?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "j_type"
    Then call the capability "chrome"
      | with.op | agent_step |
      | with.target_id | ${state.browser_target} |
      | with.decision | ${with.dec} |
      | project.decision | /dec |
      | project.step_action | /dec/operation |
      | project.step_index | /dec/element |
      | project.step_name | /dec/name |
      | project.step_note | /dec/note |
      | project.step_rationale | /dec/rationale |
      | project.step_href | /dec/href |
      | project.step_stayed | /dec/stayed_in_tab |
      | project.act | /result |
      | chain.dec.capability | planner |
      | chain.dec.with.operation | TYPE_TEXT |
      | chain.dec.with.roles | ["combobox","textbox"] |
      | chain.dec.with.match | ["请输入搜索文字"] |
      | chain.dec.with.text | DDR5 服务器内存 |
      | chain.dec.with.note | 填选择框：搜索 DDR5 服务器内存 |
      | chain.dec.with.op | find |
      | chain.dec.with.target_id | ${state.browser_target} |

  Scenario: j_type
    Given a choice question "go" asks "Record this round in the tour journal and continue?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "click_search"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/tour.jsonl |
      | with.text | "- ${state.step_rationale}\n" |
      | project.journal_bytes | /bytes |

  Scenario: click_search
    Given a choice question "go" asks "Click the 搜索 button to run the search?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "j_search"
    Then call the capability "chrome"
      | with.op | agent_step |
      | with.target_id | ${state.browser_target} |
      | with.decision | ${with.dec} |
      | project.decision | /dec |
      | project.step_action | /dec/operation |
      | project.step_index | /dec/element |
      | project.step_name | /dec/name |
      | project.step_note | /dec/note |
      | project.step_rationale | /dec/rationale |
      | project.step_href | /dec/href |
      | project.step_stayed | /dec/stayed_in_tab |
      | project.act | /result |
      | chain.dec.capability | planner |
      | chain.dec.with.operation | CLICK |
      | chain.dec.with.roles | ["button"] |
      | chain.dec.with.match | ["搜索"] |
      | chain.dec.with.note | 点搜索（站点会另开新页；stay_in_tab 让它落在本标签页） |
      | chain.dec.with.op | find |
      | chain.dec.with.target_id | ${state.browser_target} |
      | chain.dec.with.stay_in_tab | true |

  Scenario: j_search
    Given a choice question "go" asks "Record this round in the tour journal and continue?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "dismiss2"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/tour.jsonl |
      | with.text | "- ${state.step_rationale}\n" |
      | project.journal_bytes | /bytes |

  Scenario: guard_results
    Given a choice question "go" asks "Wait out an anti-bot wall (滑块/验证码/访问太频繁) on https://s.taobao.com/ before acting?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "sort_sales"
    Then call the capability "planner"
      | with.op | guard |
      | with.target_id | ${state.browser_target} |
      | with.url | https://s.taobao.com/ |
      | with.wait_ms | 120000 |
      | project.wall | /wall |
      | project.guard_attempts | /attempts |
      | project.guard_waited_ms | /waited_ms |

  Scenario: dismiss2
    Given a choice question "go" asks "搜索后可能又弹出横幅，先关掉再点筛选项?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "guard_results"
    Then call the capability "planner"
      | with.op | dismiss |
      | with.target_id | ${state.browser_target} |
      | project.dismissed | /overlays |
      | project.still_blocking | /still_blocking |

  Scenario: sort_sales
    Given a choice question "go" asks "筛选项选择：点 销量 标签?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "j_sort"
    Then call the capability "chrome"
      | with.op | agent_step |
      | with.target_id | ${state.browser_target} |
      | with.decision | ${with.dec} |
      | project.decision | /dec |
      | project.step_action | /dec/operation |
      | project.step_index | /dec/element |
      | project.step_name | /dec/name |
      | project.step_note | /dec/note |
      | project.step_rationale | /dec/rationale |
      | project.step_href | /dec/href |
      | project.step_stayed | /dec/stayed_in_tab |
      | project.act | /result |
      | chain.dec.capability | planner |
      | chain.dec.with.operation | CLICK |
      | chain.dec.with.roles | ["tab"] |
      | chain.dec.with.match | ["销量"] |
      | chain.dec.with.note | 筛选项选择：点 销量 标签 |
      | chain.dec.with.op | find |
      | chain.dec.with.target_id | ${state.browser_target} |
      | chain.dec.with.stay_in_tab | true |
      | chain.dec.with.retry_ms | 8000 |

  Scenario: j_sort
    Given a choice question "go" asks "记录排序这一步?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "observe_p1"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/sweep_tour.jsonl |
      | with.text | "- ${state.step_rationale}\n" |
      | project.journal_bytes | /bytes |

  Scenario: observe_p1
    Given a choice question "go" asks "Draw the numbered overlay on the results page and read it as a numbered table?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "scroll_p1"
    Then call the capability "planner"
      | with.op | observe |
      | with.target_id | ${state.browser_target} |
      | project.p1_url | /url |
      | project.p1_title | /title |
      | project.p1_elements | /elements |
      | project.p1_badges | /badges |
      | project.p1_sample | /sample |
      | project.p1_money | /money |

  Scenario: scroll_p1
    Given a choice question "go" asks "Read down the first results page (so the numbered pager enters the viewport), then paginate?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "next_p2"
    Then call the capability "chrome"
      | with.op | scroll |
      | with.target_id | ${state.browser_target} |
      | with.amount | 4000 |
      | project.scrolled_to | /ok |

  Scenario: next_p2
    Given a choice question "go" asks "Turn to the next page of results?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "j_next2"
    Then call the capability "chrome"
      | with.op | agent_step |
      | with.target_id | ${state.browser_target} |
      | with.decision | ${with.dec} |
      | project.decision | /dec |
      | project.step_action | /dec/operation |
      | project.step_index | /dec/element |
      | project.step_name | /dec/name |
      | project.step_note | /dec/note |
      | project.step_rationale | /dec/rationale |
      | project.step_href | /dec/href |
      | project.step_stayed | /dec/stayed_in_tab |
      | project.act | /result |
      | chain.dec.capability | planner |
      | chain.dec.with.operation | CLICK |
      | chain.dec.with.roles | ["button"] |
      | chain.dec.with.match | ["下一页"] |
      | chain.dec.with.note | 多页浏览：翻到第 2 页 |
      | chain.dec.with.op | find |
      | chain.dec.with.target_id | ${state.browser_target} |
      | chain.dec.with.retry_ms | 6000 |
      | chain.dec.with.selector | "button[aria-label*=\"下一页\"]" |

  Scenario: j_next2
    Given a choice question "go" asks "Record this round in the tour journal and continue?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "observe_p2"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/tour.jsonl |
      | with.text | "- ${state.step_rationale}\n" |
      | project.journal_bytes | /bytes |

  Scenario: observe_p2
    Given a choice question "go" asks "Number the second page and read it?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "scroll_p2"
    Then call the capability "planner"
      | with.op | observe |
      | with.target_id | ${state.browser_target} |
      | with.prev_url | ${state.p1_url} |
      | project.p2_url | /url |
      | project.p2_title | /title |
      | project.p2_elements | /elements |
      | project.p2_sample | /sample |

  Scenario: scroll_p2
    Given a choice question "go" asks "Read down the second results page, then paginate again?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "next_p3"
    Then call the capability "chrome"
      | with.op | scroll |
      | with.target_id | ${state.browser_target} |
      | with.amount | 4000 |
      | project.scrolled_to | /ok |

  Scenario: next_p3
    Given a choice question "go" asks "Turn to the next page of results again?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "j_next3"
    Then call the capability "chrome"
      | with.op | agent_step |
      | with.target_id | ${state.browser_target} |
      | with.decision | ${with.dec} |
      | project.decision | /dec |
      | project.step_action | /dec/operation |
      | project.step_index | /dec/element |
      | project.step_name | /dec/name |
      | project.step_note | /dec/note |
      | project.step_rationale | /dec/rationale |
      | project.step_href | /dec/href |
      | project.step_stayed | /dec/stayed_in_tab |
      | project.act | /result |
      | chain.dec.capability | planner |
      | chain.dec.with.operation | CLICK |
      | chain.dec.with.roles | ["button"] |
      | chain.dec.with.match | ["下一页"] |
      | chain.dec.with.note | 多页浏览：翻到第 3 页 |
      | chain.dec.with.op | find |
      | chain.dec.with.target_id | ${state.browser_target} |
      | chain.dec.with.retry_ms | 6000 |
      | chain.dec.with.selector | "button[aria-label*=\"下一页\"]" |

  Scenario: j_next3
    Given a choice question "go" asks "Record this round in the tour journal and continue?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "observe_p3"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/tour.jsonl |
      | with.text | "- ${state.step_rationale}\n" |
      | project.journal_bytes | /bytes |

  Scenario: observe_p3
    Given a choice question "go" asks "Number the third page and read it?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "sweep"
    Then call the capability "planner"
      | with.op | observe |
      | with.target_id | ${state.browser_target} |
      | with.prev_url | ${state.p2_url} |
      | project.p3_url | /url |
      | project.p3_title | /title |
      | project.p3_elements | /elements |
      | project.p3_sample | /sample |

  Scenario: sweep
    Given a choice question "go" asks "把第 3 页的 20 张商品卡一个一个打开、读详情、打分排序，选出最推荐的?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "j_sweep"
    Then call the capability "planner"
      | with.op | sweep |
      | with.target_id | ${state.browser_target} |
      | with.list_url | ${state.p3_url} |
      | with.n | 20 |
      | with.results | ${env.HOME}/tmp/jev-deep/sweep.jsonl |
      | project.sweep_total | /total_cards |
      | project.sweep_opened | /opened |
      | project.sweep_best_index | /best_index |
      | project.best_href | /best_href |
      | project.best_title | /best_title |
      | project.best_price | /best_price |
      | project.best_score | /best_score |
      | project.best_spec | /best_spec |

  Scenario: j_sweep
    Given a choice question "go" asks "把这次比较的结论记一行?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "open_best"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/sweep_tour.jsonl |
      | with.text | "- 比较了 ${state.sweep_opened} 个商品；最推荐：${state.best_title}（score ${state.best_score}, ¥${state.best_price}）\n" |
      | project.journal_bytes | /bytes |

  Scenario: open_best
    Given a choice question "go" asks "打开最推荐商品详情页并停留?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "observe_best"
    Then call the capability "planner"
      | with.op | visit |
      | with.target_id | ${state.browser_target} |
      | with.url | ${state.best_href} |
      | project.best_url | /url |
      | project.best_landing_title | /title |

  Scenario: observe_best
    Given a choice question "go" asks "给最推荐详情页画序号并读价格/规格，然后停?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "planner"
      | with.op | observe |
      | with.target_id | ${state.browser_target} |
      | project.final_url | /url |
      | project.final_title | /title |
      | project.final_elements | /elements |
      | project.final_sample | /sample |
      | project.final_money | /money |
      | project.final_price_hint | /price_hint |
