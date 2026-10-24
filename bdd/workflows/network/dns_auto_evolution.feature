@decision
Feature: dns_auto_evolution
  Decide, per candidate domain harvested from the bypass router's DNS logs,
  whether it must be pinned to domestic resolution (the auto-evolution step of
  the Feishu-style fix). The harness fills five evidence fields; the graph
  reads nothing else:

    - ev_domain    the candidate domain, e.g. "www.feishu.cn".
    - ev_drift     "overseas" (clash DNS returned an overseas edge while a
                   direct domestic DoH returned a domestic one), "none" (no
                   drift), or "unknown".
    - ev_kind      "domestic" (.cn / .com.cn / domestic CDN shapes / geosite:cn
                   hint), "foreign", or "unknown".
    - ev_pinned    "yes" (already in nameserver-policy / fake-ip-filter /
                   DIRECT rules) or "no".
    - ev_outbound  "proxy" (clash returned a fake-ip 198.18.x for it, i.e. the
                   rule sends it through the proxy), "direct", or "unknown".

  Decision ladder (first-fail):
    1. drift     -- no drift: nothing to do. unknown: needs a human look.
    2. kind      -- foreign by design: do not touch. unknown: review.
    3. pinned    -- already pinned: nothing new to apply.
    4. outbound  -- proxy: pin DNS + fake-ip-filter + DIRECT rule.
                   direct/unknown: pin DNS + fake-ip-filter only.

  Emitted labels: SKIP_NO_DRIFT, SKIP_FOREIGN, SKIP_ALREADY, REVIEW,
  ADD_DNS_PIN, ADD_DNS_PIN_DIRECT.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: drift
    Given a choice question "drift_state" asks "Did clash DNS return an overseas edge while a direct domestic DoH returned a domestic one? overseas = real drift worth fixing; none = both agree; unknown = the cross-check could not be completed."
      | answer   | meaning |
      | overseas | clash answered with an overseas IP; direct domestic DoH answered domestic. |
      | none     | clash and direct DoH agree; no drift. |
      | unknown  | The cross-check could not be completed. |
    And answer by rules on the field "ev_drift" with hit 0.9 and fallback "none"
      | label    | needles |
      | overseas | "overseas", "海外", "drift", "漂移" |
      | unknown  | "unknown", "无法", "failed" |
      | none     | "none", "agree", "一致" |
    When the answer "overseas" goes to "kind"
    And any other answer goes to "STOP"
    Then the primary question is "drift_state"
    Then label "SKIP_NO_DRIFT" when the answer to "drift_state" is "none"
    And label "REVIEW" when the answer to "drift_state" is "unknown"
    And label "PROBE_DRIFT_OK" otherwise

  Scenario: kind
    Given a choice question "kind_state" asks "Is this domain a domestic service by shape (.cn / .com.cn / domestic CDN suffix / geosite:cn)? foreign = it is meant to be overseas; unknown = cannot tell."
      | answer   | meaning |
      | domestic | The domain shape says it is a domestic service. |
      | foreign  | The domain is a foreign service by design. |
      | unknown  | The domain's intent cannot be determined. |
    And answer by rules on the field "ev_kind" with hit 0.9 and fallback "unknown"
      | label    | needles |
      | domestic | "domestic", "国内", "cn", "geosite" |
      | foreign  | "foreign", "海外服务", "google", "github", "openai" |
      | unknown  | "unknown", "不确定" |
    When the answer "domestic" goes to "pinned"
    And any other answer goes to "STOP"
    Then the primary question is "kind_state"
    Then label "SKIP_FOREIGN" when the answer to "kind_state" is "foreign"
    And label "REVIEW" when the answer to "kind_state" is "unknown"
    And label "PROBE_KIND_OK" otherwise

  Scenario: pinned
    Given a choice question "pinned_state" asks "Is this domain already covered by the DNS optimization (nameserver-policy / fake-ip-filter / DIRECT rules)? yes = already applied, nothing new."
      | answer | meaning |
      | yes    | Already pinned in at least one of the optimization lists. |
      | no     | Not pinned anywhere yet. |
    And answer by rules on the field "ev_pinned" with hit 0.9 and fallback "no"
      | label | needles |
      | yes   | "yes", "pinned", "已配置", "已钉" |
      | no    | "no", "absent", "未配置" |
    When the answer "no" goes to "outbound"
    And any other answer goes to "STOP"
    Then the primary question is "pinned_state"
    Then label "SKIP_ALREADY" when the answer to "pinned_state" is "yes"
    And label "PROBE_PINNED_OK" otherwise

  Scenario: outbound
    Given a choice question "outbound_state" asks "What outbound does this domain's rule use? proxy (clash returned a fake-ip 198.18.x for it) means it also needs a DIRECT rule; direct/unknown means DNS pinning suffices."
      | answer  | meaning |
      | proxy   | The rule sends this domain through the proxy (fake-ip answer). |
      | direct  | The rule already sends this domain direct. |
      | unknown | The outbound cannot be determined. |
    And answer by rules on the field "ev_outbound" with hit 0.9 and fallback "unknown"
      | label   | needles |
      | proxy   | "proxy", "代理", "fake-ip", "198.18" |
      | direct  | "direct", "直连" |
      | unknown | "unknown", "不确定" |
    When any other answer goes to "STOP"
    Then the primary question is "outbound_state"
    Then label "ADD_DNS_PIN_DIRECT" when the answer to "outbound_state" is "proxy"
    And label "ADD_DNS_PIN" otherwise
