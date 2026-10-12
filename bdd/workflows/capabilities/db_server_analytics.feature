@decision
Feature: db_server_analytics

  Background:
    Given the workflow runs at most 10 iterations

  Scenario: reset
    Given a choice question "go" asks "Reset the rows in SQLite (via the daemon)?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "report"
    Then call the capability "store_rw"
      | with.op      | exec |
      | with.statements      | ["CREATE TABLE IF NOT EXISTS sales (id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount REAL NOT NULL)", "DELETE FROM sales", "INSERT INTO sales (region, amount) VALUES ('east', 10.5), ('west', 20.0), ('east', 5.25), ('west', 1.5)"] |
      | project.ok  | /ok |
      | project.http_status  | /http_status |

  Scenario: report
    Given a choice question "go" asks "Aggregate with DuckDB (through the daemon)?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "persist"
    Then call the capability "store_ro"
      | with.op      | analytics |
      | with.format      | json |
      | with.sql      | SELECT region, SUM(amount) AS total, COUNT(*) AS n FROM sales GROUP BY region ORDER BY region |
      | project.rows  | /rows |
      | project.engine  | /engine |
      | project.mode  | /mode |

  Scenario: persist
    Given a choice question "go" asks "Write the aggregate back to SQLite?"
      | answer | meaning              |
      | A      | yes                            |
    When any other answer goes to "verify"
    Then call the capability "store_rw"
      | with.op      | sync |
      | with.create      | true |
      | with.into      | region_totals |
      | with.sql      | SELECT region, SUM(amount) AS total, COUNT(*) AS n FROM sales GROUP BY region |
      | project.ok  | /ok |
      | project.http_status  | /http_status |

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

