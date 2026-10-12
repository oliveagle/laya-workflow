@decision
Feature: goofish_price_model
  Port of goofish_sqlite: converted from dsl/browser/goofish_sqlite.json (BDD round-trip).

  Background:
    Given the workflow runs at most 8 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/goofish", "${env.HOME}/tmp/goofish-evolve" |
      | max_timeout_ms | 600000 |
      | max_output | 8388608 |
      | retries | 0 |
    Given the capabilities
      | browser | {"kind":"browser","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":120000,"max_text":20000} |
      | goofish | {"kind":"plugin","plugin":"websites/goofish","browser":"browser","timeout_ms":540000,"max_operations":8000000} |
      | gf_db | {"kind":"db","sqlite":"${env.HOME}/tmp/goofish/goofish.sqlite","readonly":false} |

  Scenario: collect
    Given a choice question "go" asks "采集并学习，然后写入 SQLite？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "schema"
    Then call the capability "goofish"
      | with.query | ${state.query} |
      | with.mode | evolve |
      | with.count | ${state.count} |
      | with.pages | ${state.pages} |
      | with.browse | ${state.browse} |
      | with.out_dir | ${state.out_dir} |
      | with.emit_sql | true |
      | with.markdown | false |
      | with.track | true |
      | project.mode | /mode |
      | project.found_count | /found_count |
      | project.notes | /notes |
      | project.evolve | /evolve |
      | project.sql_schema | /sql_schema |
      | project.sql_statements | /sql_statements |

  Scenario: schema
    Given a choice question "go" asks "建表？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "ingest"
    Then call the capability "gf_db"
      | with.op | exec |
      | with.statements | ${state.sql_schema} |
      | project.ok | /ok |
      | project.exit_code | /exit_code |
      | project.stderr | /stderr |

  Scenario: ingest
    Given a choice question "go" asks "写入商品/标签/价格点？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "fair"
    Then call the capability "gf_db"
      | with.op | exec |
      | with.statements | ${state.sql_statements} |
      | project.ok | /ok |
      | project.exit_code | /exit_code |
      | project.stderr | /stderr |

  Scenario: fair
    Given a choice question "go" asks "按标签看合理价格区间？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "drops"
    Then call the capability "gf_db"
      | with.op | query |
      | with.sql | SELECT t.ns, t.name, t.items, p.n AS priced, p.median, p.mad, p.low, p.high, t.spellings FROM goofish_tags t JOIN goofish_tag_price_stats p ON p.tag = t.tag WHERE p.fair IS NOT NULL ORDER BY p.n DESC, t.ns, t.name LIMIT 20 |
      | project.rows | /rows |

  Scenario: drops
    Given a choice question "go" asks "看降价幅度榜？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "trend"
    Then call the capability "gf_db"
      | with.op | analytics |
      | with.format | json |
      | with.sql | "SELECT item_id, price_at, price, prev_price, ROUND((price - prev_price) * 100.0 / NULLIF(prev_price, 0), 1) AS move_pct, CASE WHEN price < prev_price THEN 'down' ELSE 'up' END AS direction FROM (SELECT item_id, \"at\" AS price_at, price, LAG(price) OVER (PARTITION BY item_id ORDER BY at_ms) AS prev_price FROM goofish_price_points) WHERE prev_price IS NOT NULL AND price <> prev_price ORDER BY ABS(move_pct) DESC LIMIT 15" |
      | project.rows | /rows |
      | project.engine | /engine |

  Scenario: trend
    Given a choice question "go" asks "看某个标签的价格行情？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "anomalies"
    Then call the capability "gf_db"
      | with.op | query |
      | with.sql | SELECT t.name AS tag, COUNT(DISTINCT it.item_id) AS items, MIN(pp.price) AS low_seen, MAX(pp.price) AS high_seen, COUNT(pp.price) AS points FROM goofish_item_tags it JOIN goofish_tags t ON t.tag = it.tag LEFT JOIN goofish_price_points pp ON pp.item_id = it.item_id WHERE t.n_priced > 0 GROUP BY t.tag ORDER BY items DESC LIMIT 20 |
      | project.rows | /rows |

  Scenario: anomalies
    Given a choice question "go" asks "看低于/高于公平价的异常报告？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "tables"
    Then call the capability "gf_db"
      | with.op | query |
      | with.sql | WITH best AS ( SELECT it.item_id, s.tag, s.fair, s.low, s.high, s.n AS support, ROW_NUMBER() OVER (PARTITION BY it.item_id ORDER BY s.n DESC, s.tag) AS rn FROM goofish_item_tags it JOIN goofish_tag_price_stats s ON s.tag = it.tag WHERE s.fair IS NOT NULL ) SELECT i.id, substr(i.title, 1, 40) AS title, i.price, b.tag, b.fair, b.low, b.high, ROUND((i.price - b.fair) * 100.0 / NULLIF(b.fair, 0), 1) AS pct_vs_fair, b.support, CASE WHEN i.price < b.low THEN 'below_fair' WHEN i.price > b.high THEN 'above_fair' ELSE 'fair' END AS verdict FROM goofish_items i JOIN best b ON b.item_id = i.id AND b.rn = 1 WHERE i.status <> 'gone' AND (i.price < b.low OR i.price > b.high) ORDER BY ABS(pct_vs_fair) DESC LIMIT 20 |
      | project.rows | /rows |

  Scenario: tables
    Given a choice question "go" asks "看看库里有什么？"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "gf_db"
      | with.op | tables |
      | project.tables | /rows |
