@decision
Feature: taobao_jev_tour
  Port of taobao_jev_tour: converted from dsl/browser/taobao_jev_tour.json (BDD round-trip).

  Background:
    Given the workflow runs at most 80 iterations
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
    Given a choice question "go" asks "Open one scratch tab in the shared singleton browser and drive the whole tour in it?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "home"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | about:blank |
      | project.browser_target | /target_id |

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

  Scenario: dismiss
    Given a choice question "go" asks "Close any full-viewport promo/login modal that would swallow the next clicks?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "type_query"
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
      | chain.dec.with.text | DDR5 内存 16G |
      | chain.dec.with.note | 填选择框：把关键词打进搜索框 |
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
    And the answer "A" goes to "observe_p1"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/tour.jsonl |
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
    And the answer "A" goes to "open_item"
    Then call the capability "planner"
      | with.op | observe |
      | with.target_id | ${state.browser_target} |
      | with.prev_url | ${state.p2_url} |
      | project.p3_url | /url |
      | project.p3_title | /title |
      | project.p3_elements | /elements |
      | project.p3_sample | /sample |

  Scenario: open_item
    Given a choice question "go" asks "Open one product detail by clicking the numbered card (kept in this tab)?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "j_item"
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
      | chain.dec.with.roles | ["link"] |
      | chain.dec.with.href | ["item.taobao.com","detail.tmall.com"] |
      | chain.dec.with.stay_in_tab | true |
      | chain.dec.with.note | 序号选商品：打开第一个商品详情 |
      | chain.dec.with.op | find |
      | chain.dec.with.target_id | ${state.browser_target} |

  Scenario: j_item
    Given a choice question "go" asks "Record this round in the tour journal and continue?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "observe_item"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/tour.jsonl |
      | with.text | "- ${state.step_rationale}\n" |
      | project.journal_bytes | /bytes |

  Scenario: observe_item
    Given a choice question "go" asks "Number the product detail page and read its price/attributes?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "scroll_item"
    Then call the capability "planner"
      | with.op | observe |
      | with.target_id | ${state.browser_target} |
      | project.item_url | /url |
      | project.item_title | /title |
      | project.item_elements | /elements |
      | project.item_sample | /sample |
      | project.item_money | /money |
      | project.item_price_hint | /price_hint |

  Scenario: scroll_item
    Given a choice question "go" asks "Scroll the detail page down to keep reading?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "observe_item2"
    Then call the capability "chrome"
      | with.op | scroll |
      | with.target_id | ${state.browser_target} |
      | with.amount | 900 |
      | project.scrolled | /ok |

  Scenario: observe_item2
    Given a choice question "go" asks "Number the next screen of the detail page and stop?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "planner"
      | with.op | observe |
      | with.target_id | ${state.browser_target} |
      | project.item2_url | /url |
      | project.item2_title | /title |
      | project.item2_elements | /elements |
      | project.item2_sample | /sample |
      | project.item2_money | /money |
