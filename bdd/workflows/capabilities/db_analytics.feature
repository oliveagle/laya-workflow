@decision
Feature: db_analytics

  Background:
    Given the workflow runs at most 10 iterations

  Scenario: ingest
    Given a choice question "go" asks "Write the raw rows into SQLite?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "report"
    Then call the capability "store_rw"
      | with.op      | exec |
      | with.statements      | ["CREATE TABLE IF NOT EXISTS sales (id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount REAL NOT NULL)", "DELETE FROM sales", "INSERT INTO sales (region, amount) VALUES ('east', 10.5), ('west', 20.0), ('east', 5.25), ('west', 1.5)"] |
      | project.ok  | /ok |
      | project.exit_code  | /exit_code |
      | project.stderr  | /stderr |

  Scenario: report
    Given a choice question "go" asks "Aggregate the sales with DuckDB?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "persist"
    Then call the capability "store_ro"
      | with.op      | analytics |
      | with.format      | json |
      | with.sql      | SELECT region, SUM(amount) AS total, COUNT(*) AS n FROM sales GROUP BY region ORDER BY region |
      | project.rows  | /rows |
      | project.engine  | /engine |

  Scenario: persist
    Given a choice question "go" asks "Write the aggregate back to SQLite as an ACID table?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "verify"
    Then call the capability "store_rw"
      | with.op      | sync |
      | with.create      | true |
      | with.into      | region_totals |
      | with.sql      | SELECT region, SUM(amount) AS total, COUNT(*) AS n FROM sales GROUP BY region |
      | project.ok  | /ok |
      | project.exit_code  | /exit_code |
      | project.stderr  | /stderr |

  Scenario: verify
    Given a choice question "go" asks "Read the materialised table back?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "inspect"
    Then call the capability "store_ro"
      | with.op      | query |
      | with.sql      | SELECT region, total, n FROM region_totals ORDER BY region |
      | project.rows  | /rows |

  Scenario: inspect
    Given a choice question "go" asks "List what each engine sees?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "STOP"
    Then call the capability "store_ro"
      | with.op      | tables |
      | project.engines  | /rows |

