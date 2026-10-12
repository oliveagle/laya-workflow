@decision
Feature: laya_mem_persist
  Port of persist_memory: converted from dsl/laya_mem/persist_memory.json (BDD round-trip).

  Background:
    Given the workflow runs at most 10 iterations
    Given the policy
      | allow_exec | true |
      | allow_paths | "${env.LAYA_WORK_DIR}" |
      | max_output | 262144 |
    Given the capabilities
      | mem_db | {"kind":"db","sqlite":"${env.LAYA_WORK_DIR}/laya_mem.sqlite","readonly":false} |

  Scenario: reset
    Given a choice question "go" asks "Reset the Laya-Mem tables?"
      | answer | meaning |
      | A | yes |
    When any other answer goes to "persist"
    Then call the capability "mem_db"
      | with.op | exec |
      | with.statements | ["CREATE TABLE IF NOT EXISTS memories (id INTEGER PRIMARY KEY, content TEXT NOT NULL, ts TEXT, entities TEXT, type_scores TEXT NOT NULL)","CREATE TABLE IF NOT EXISTS relations (id INTEGER PRIMARY KEY, source TEXT, target TEXT, link_type TEXT, probability REAL)","DELETE FROM memories","DELETE FROM relations"] |
      | project.ok | /ok |
      | project.exit_code | /exit_code |
      | project.stderr | /stderr |

  Scenario: persist
    Given a choice question "go" asks "Insert the observation into `memories`?"
      | answer | meaning |
      | A | yes |
    When any other answer goes to "link"
    Then call the capability "mem_db"
      | with.op | exec |
      | with.statements | ["INSERT INTO memories (content, ts, entities, type_scores) VALUES ('Alice prefers concise explanations.', '2026-09-02', '[\"Alice\"]', '{\"episodic\":0.1,\"semantic\":0.2,\"procedural\":0.1,\"preference\":0.9}')"] |
      | project.ok | /ok |
      | project.exit_code | /exit_code |
      | project.stderr | /stderr |

  Scenario: link
    Given a choice question "go" asks "Insert a relation row?"
      | answer | meaning |
      | A | yes |
    When any other answer goes to "verify"
    Then call the capability "mem_db"
      | with.op | exec |
      | with.statements | ["INSERT INTO relations (source, target, link_type, probability) VALUES ('m1', 'm2', 'SEMANTIC', 0.7)"] |
      | project.ok | /ok |
      | project.exit_code | /exit_code |
      | project.stderr | /stderr |

  Scenario: verify
    Given a choice question "go" asks "Read the rows back?"
      | answer | meaning |
      | A | yes |
    When any other answer goes to "inspect"
    Then call the capability "mem_db"
      | with.op | query |
      | with.sql | SELECT id, content, type_scores FROM memories ORDER BY id |
      | project.rows | /rows |
      | project.exit_code | /exit_code |

  Scenario: inspect
    Given a choice question "go" asks "List relations?"
      | answer | meaning |
      | A | yes |
    When any other answer goes to "STOP"
    Then call the capability "mem_db"
      | with.op | query |
      | with.sql | SELECT source, target, link_type, probability FROM relations ORDER BY id |
      | project.relations | /rows |
