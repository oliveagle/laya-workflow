@decision
Feature: data_pipeline_local
  Port of data_pipeline_local: converted from dsl/capabilities/data_pipeline_local.json (BDD round-trip).

  Background:
    Given the workflow runs at most 10 iterations
    Given the policy
      | allow_exec | true |
      | allow_paths | "${env.LAYA_WORK_DIR}" |
      | max_timeout_ms | 8000 |
      | max_output | 65536 |
      | retries | 0 |
    Given the capabilities
      | clock | {"kind":"datetime","op":"now"} |
      | stamp | {"kind":"datetime","op":"format","format":"%Y-%m-%d %H:%M:%S"} |
      | slug | {"kind":"text","op":"extract","pattern":"[a-z0-9-]+"} |
      | report | {"kind":"file","op":"write","max_bytes":65536} |
      | disk | {"kind":"shell","command":"du -sk \"${state.work_dir}\" \u007c awk '{print $1}'","platforms":["unix"]} |
      | raw | {"kind":"passthrough"} |

  Scenario: stamp
    Given a choice question "kind_of_record" asks "What kind of record is this?"
      | answer | meaning |
      | note | a short note or memo |
      | incident | an incident or outage report |
      | invoice | a billing document |
    When any other answer goes to "enrich"
    Then call the capability "stamp"
      | with.epoch | ${with.clock.epoch} |
      | project.stamped | /text |
      | chain.clock.capability | clock |

  Scenario: enrich
    Given a choice question "sensitivity" asks "Does this record contain sensitive data that should not be written to disk as-is?"
      | answer | meaning |
      | A | no, it is safe to persist |
      | B | yes, it contains sensitive data |
    When any other answer goes to "STOP"
    Then label "WITHHOLD" when the answer is "B" with at least 0.5 confidence
    And label "PERSIST" otherwise
