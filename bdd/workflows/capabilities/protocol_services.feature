@decision
Feature: protocol_services
  Port of protocol_services: converted from dsl/capabilities/protocol_services.json (BDD round-trip).

  Background:
    Given the workflow runs at most 20 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 8000 |
      | max_output | 65536 |
      | retries | 0 |
    Given the capabilities
      | ping_upstream | {"kind":"tcp","host":"127.0.0.1","port":6380,"timeout_ms":3000} |
      | echo_udp | {"kind":"udp","host":"127.0.0.1","port":9999,"timeout_ms":3000} |
      | cache_rw | {"kind":"redis","host":"127.0.0.1","port":6380,"timeout_ms":3000} |
      | bus_publish | {"kind":"nats","host":"127.0.0.1","port":4223,"timeout_ms":3000} |
      | iot_publish | {"kind":"mqtt","host":"127.0.0.1","port":1884,"client_id":"laya-workflow","timeout_ms":3000} |
      | notify_mail | {"kind":"smtp","host":"127.0.0.1","port":2526,"from":"workflow@example.com","username":"${env.LAYA_SMTP_USER}","password":"${env.LAYA_SMTP_PASS}","timeout_ms":5000} |
      | objects | {"kind":"s3","endpoint":"http://127.0.0.1:9000","bucket":"bucket","timeout_ms":5000} |
      | metrics_endpoint | {"kind":"prometheus","url":"http://127.0.0.1:9091/metrics","timeout_ms":5000} |
      | event_stream | {"kind":"kafka","url":"http://127.0.0.1:8083","topic":"events","timeout_ms":5000} |

  Scenario: route
    Given a choice question "channel" asks "Which protocol or service should carry this action?"
      | answer | meaning |
      | raw_socket | a raw TCP/UDP socket exchange |
      | cache | a key/value cache read or write |
      | message_bus | a pub/sub message bus |
      | notification | an outbound notification |
      | object_store | an object storage operation |
      | observability | metrics or observability data |
    And the answer comes from the field "channel"
    When any other answer goes to "STOP"
    And the answer "raw_socket" goes to "do_tcp"
    And the answer "cache" goes to "do_redis"
    And the answer "message_bus" goes to "do_bus"
    And the answer "notification" goes to "do_mail"
    And the answer "object_store" goes to "do_s3"
    And the answer "observability" goes to "do_metrics"
    Then keep the answer to "channel"
    Then allow up to 1 retries

  Scenario: do_tcp
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "ping_upstream"
      | with.send | "*1\r\n$4\r\nPING\r\n" |
      | project.tcp_reply | /reply |
      | project.udp_reply | /udp/reply |
      | chain.udp.capability | echo_udp |
      | chain.udp.with.send | probe |

  Scenario: do_redis
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "cache_rw"
      | with.op | set |
      | with.key | workflow:last |
      | with.value | ${state.text} |
      | project.redis_reply | /reply |

  Scenario: do_bus
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "bus_publish"
      | with.subject | workflow.events |
      | with.message | ${state.text} |
      | project.nats_bytes | /sent_bytes |
      | project.mqtt_connected | /mqtt/connected |
      | chain.mqtt.capability | iot_publish |
      | chain.mqtt.with.topic | workflow/events |
      | chain.mqtt.with.message | ${state.text} |

  Scenario: do_mail
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "notify_mail"
      | with.to | ["ops@example.com"] |
      | with.subject | workflow notice |
      | with.body | ${state.text} |
      | project.mail_recipients | /to |
      | project.mail_authed | /authed |

  Scenario: do_s3
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "objects"
      | with.op | put |
      | with.key | artifacts/run.txt |
      | with.body | ${state.text} |
      | project.s3_status | /status |
      | project.s3_key | /key |

  Scenario: do_metrics
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "metrics_endpoint"
      | project.metric_samples | /count |
      | project.metric_mode | /mode |
