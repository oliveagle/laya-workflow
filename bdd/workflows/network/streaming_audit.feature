@decision
Feature: streaming_audit
  Audit one streaming domain: domestic streaming must go DIRECT (a proxy hop
  adds cross-border latency to a domestic CDN and makes it unusable), foreign
  streaming must ride the dedicated streaming (unlock) group.

  Evidence fields:
    - ev_domain    the probe target, e.g. "v.qq.com" or "netflix.com".
    - ev_kind      "domestic-stream" or "foreign-stream".
    - ev_outbound  observed outbound: "direct" / "proxy" (the general group) /
                   "stream-group" (the dedicated streaming group) / "unknown".
    - ev_latency   "fast" (< 1s) / "slow" (>= 1s) / "timeout".

  Emitted labels: OK_DOMESTIC_DIRECT, OK_FOREIGN_STREAM, FIX_DOMESTIC_STREAM,
  FIX_STREAM_GROUP, FIX_PROXY_RULE, REVIEW.

  Background:
    Given the workflow runs at most 4 iterations

  Scenario: kind
    Given a choice question "kind_state" asks "Is this a domestic streaming service or a foreign one?"
      | answer          | meaning |
      | domestic-stream | A CN streaming service (iqiyi / tencent video / youku / bilibili / mgtv / douyin / kuaishou / netease music ...). |
      | foreign-stream  | A non-CN streaming service (netflix / youtube / disney+ / hbo / prime / hulu / spotify ...). |
      | unknown         | The category cannot be determined. |
    And answer by rules on the field "ev_kind" with hit 0.9 and fallback "unknown"
      | label           | needles |
      | domestic-stream | "domestic-stream", "国内", "domestic" |
      | foreign-stream  | "foreign-stream", "国外", "foreign" |
      | unknown         | "unknown", "不确定" |
    When the answer "domestic-stream" goes to "dom_path"
    And the answer "foreign-stream" goes to "foreign_path"
    And any other answer goes to "STOP"
    Then the primary question is "kind_state"
    Then label "REVIEW" when the answer to "kind_state" is "unknown"
    And label "PROBE_KIND_OK" otherwise

  Scenario: dom_path
    Given a choice question "dom_outbound" asks "How does a domestic streaming domain get out? direct is the only acceptable path (proxy = cross-border detour = slow)."
      | answer  | meaning |
      | direct  | Matched a DIRECT rule. |
      | proxy   | Matched a proxy group. |
      | unknown | No match line captured. |
    And answer by rules on the field "ev_outbound" with hit 0.9 and fallback "unknown"
      | label   | needles |
      | direct  | "direct", "直连" |
      | proxy   | "proxy", "代理" |
      | unknown | "unknown", "不确定" |
    And a choice question "dom_latency" asks "Is the domestic streaming path fast (< 1s)?"
      | answer  | meaning |
      | fast    | TTFB under 1 second. |
      | slow    | TTFB 1 second or more. |
      | timeout | No response. |
    And answer by rules on the field "ev_latency" with hit 0.9 and fallback "timeout"
      | label   | needles |
      | fast    | "fast", "快" |
      | slow    | "slow", "慢" |
      | timeout | "timeout", "超时" |
    When any other answer goes to "STOP"
    Then the primary question is "dom_outbound"
    Then label "OK_DOMESTIC_DIRECT" when the answer to "dom_outbound" is "direct" and the answer to "dom_latency" is "fast"
    And label "FIX_DOMESTIC_STREAM" when the answer to "dom_outbound" is "proxy"
    And label "REVIEW" otherwise

  Scenario: foreign_path
    Given a choice question "foreign_outbound" asks "How does a foreign streaming domain get out? It should ride the dedicated streaming (unlock) group, not the general proxy group."
      | answer       | meaning |
      | stream-group | Matched the dedicated streaming group. |
      | proxy        | Matched the general proxy group. |
      | direct       | Matched DIRECT (foreign streaming cannot work direct). |
      | unknown      | No match line captured. |
    And answer by rules on the field "ev_outbound" with hit 0.9 and fallback "unknown"
      | label        | needles |
      | stream-group | "stream-group", "流媒体" |
      | proxy        | "proxy", "代理" |
      | direct       | "direct", "直连" |
      | unknown      | "unknown", "不确定" |
    And a choice question "foreign_latency" asks "Is the foreign streaming path fast (< 1s)?"
      | answer  | meaning |
      | fast    | TTFB under 1 second. |
      | slow    | TTFB 1 second or more. |
      | timeout | No response. |
    And answer by rules on the field "ev_latency" with hit 0.9 and fallback "timeout"
      | label   | needles |
      | fast    | "fast", "快" |
      | slow    | "slow", "慢" |
      | timeout | "timeout", "超时" |
    When any other answer goes to "STOP"
    Then the primary question is "foreign_outbound"
    Then label "OK_FOREIGN_STREAM" when the answer to "foreign_outbound" is "stream-group" and the answer to "foreign_latency" is "fast"
    And label "FIX_STREAM_GROUP" when the answer to "foreign_outbound" is "proxy"
    And label "FIX_PROXY_RULE" when the answer to "foreign_outbound" is "direct"
    And label "REVIEW" otherwise
