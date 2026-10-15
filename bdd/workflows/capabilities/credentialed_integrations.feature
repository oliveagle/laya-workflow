@decision
Feature: credentialed_integrations
  Port of credentialed_integrations: converted from dsl/capabilities/credentialed_integrations.json (BDD round-trip).

  Background:
    Given the workflow runs at most 15 iterations
    Given the policy
      | allow_exec | false |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 8000 |
      | max_output | 32768 |
      | retries | 1 |
      | allow_paths | "${env.LAYA_STORE_DIR}" |
    Given the capabilities
      | signed_notify | {"kind":"webhook","url":"http://127.0.0.1:8791/webhook","sign_header":"x-signature","sign_secret":"${secret.LAYA_WEBHOOK_SECRET}","timeout_ms":4000} |
      | vendor_api | {"kind":"http","method":"POST","url":"http://127.0.0.1:8791/score","headers":{"content-type":"application/json","authorization":"Bearer ${secret.LAYA_VENDOR_TOKEN}"},"body":{"text":"${state.text}"},"timeout_ms":4000} |
      | mailer | {"kind":"smtp","host":"127.0.0.1","port":2526,"from":"workflow@example.com","username":"${secret.LAYA_SMTP_USER}","password":"${secret.LAYA_SMTP_PASS}","timeout_ms":5000} |
      | cached_token | {"kind":"cache","op":"set","path":"${env.LAYA_STORE_DIR}/tokens.json","ttl_secs":900} |

  Scenario: route
    Given a choice question "channel" asks "Which credentialed integration should this request use?"
      | answer | meaning |
      | vendor | a third-party API that needs an API token |
      | notify | an outbound webhook that must be signed with a shared secret |
      | email | an SMTP send that needs a username and password |
    When any other answer goes to "STOP"
    And the answer "vendor" goes to "call_vendor"
    And the answer "notify" goes to "send_hook"
    And the answer "email" goes to "send_mail"
    Then keep the answer to "channel"
    Then allow up to 1 retries

  Scenario: call_vendor
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "vendor_api"
      | project.vendor_status | /status |
      | project.vendor_label | /body/label |

  Scenario: send_hook
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "signed_notify"
      | with.event | routed |
      | with.payload | {"text":"${state.text}"} |
      | project.hook_status | /status |
      | project.hook_signed | /signed |

  Scenario: send_mail
    Given a choice question "ok" asks "proceed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "mailer"
      | with.to | ["ops@example.com"] |
      | with.subject | workflow |
      | with.body | ${state.text} |
      | project.mail_authed | /authed |
      | project.token_ttl_set | /tok/ttl_secs |
      | chain.tok.capability | cached_token |
      | chain.tok.with.key | mailer |
      | chain.tok.with.value | issued |
