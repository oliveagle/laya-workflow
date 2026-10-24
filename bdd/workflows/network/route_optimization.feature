@decision
Feature: route_optimization
  Turn an access_path_diag verdict into one concrete routing action. The graph
  owns the decision order: fix the DNS policy first (it picks the edge), then
  the direct rule (it picks the path), and only escalate when both are already
  in place and the path is still slow.

  Inputs (filled by the harness from the diagnosis):
    - ev_fault      "none" / "dns_split" / "overseas_edge" / "proxy_rule" /
                    "path_quality" (the access_path_diag label, lowercased)
    - ev_dns_policy "present" / "absent" -- is the domain pinned to a domestic
                    resolver (nameserver-policy / fake-ip excluded)?
    - ev_rule       "present" / "absent" -- is there already a DIRECT rule for
                    the domain in the proxy config?

  Emitted labels (the optimization advice, in fix order):
    KEEP_AS_IS        -- nothing to do.
    PIN_DOMESTIC_DNS  -- pin the domain to a domestic resolver (223.5.5.5 /
                         119.29.29.29), exclude it from fake-ip.
    ADD_DIRECT_RULE   -- add the domain to the DIRECT rule set (bypass proxy).
    ESCALATE_UPSTREAM -- DNS and DIRECT are already right; the remaining
                         slowness is link/upstream quality, escalate.

  Reference advice for the Feishu case (measured 2026-10-24):
    domains   feishu.cn, feishucdn.com, bytedns1.com, cdnbuild.net,
              cdngslb.com, queniurc.com, larksuite.com
    dns       223.5.5.5 / 223.6.6.6 (AliDNS), 119.29.29.29 (DNSPod)
    verify    re-run access_path_diag: dns_split -> consistent, edge -> domestic,
              rtt -> fast (TTFB < 500ms)

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: fault
    Given a choice question "fault_kind" asks "What did access_path_diag conclude for this domain? none = healthy; dns_split = :53 differs from DoH; overseas_edge = domestic service served from an overseas edge; proxy_rule = a domestic domain is proxied; path_quality = clean path but RTT over budget."
      | answer       | meaning |
      | none         | The path is healthy; no optimization needed. |
      | dns_split    | The system resolver and DoH disagree. |
      | overseas_edge | The resolved CDN edge is outside mainland China. |
      | proxy_rule   | The domain's traffic goes through a proxy. |
      | path_quality | DNS and path are clean but RTT exceeds budget. |
    And answer by rules on the field "ev_fault" with hit 0.9 and fallback "none"
      | label         | needles |
      | dns_split     | "dns_split", "dns split", "不一致" |
      | overseas_edge | "overseas", "海外", "hk", "香港", "tokyo", "东京" |
      | proxy_rule    | "proxy", "代理", "tun", "旁路由" |
      | path_quality  | "path_quality", "slow", "慢", "timeout" |
      | none          | "none", "ok", "healthy", "正常" |
    When the answer "dns_split" goes to "dns_policy"
    And the answer "overseas_edge" goes to "dns_policy"
    And the answer "proxy_rule" goes to "rule_state"
    And the answer "path_quality" goes to "rule_state"
    And any other answer goes to "STOP"
    Then the primary question is "fault_kind"
    Then label "KEEP_AS_IS" when the answer to "fault_kind" is "none"
    And label "PROBE_FAULT_OK" otherwise

  Scenario: dns_policy
    Given a choice question "dns_policy_state" asks "Is this domain already pinned to a domestic resolver (nameserver-policy / DNS split), and excluded from fake-ip? Pinning DNS first is what makes the edge domestic; a direct rule alone cannot fix a foreign edge."
      | answer  | meaning |
      | present | The domain already resolves through a domestic resolver. |
      | absent  | The domain still resolves through the proxy DNS / fake-ip. |
    And answer by rules on the field "ev_dns_policy" with hit 0.9 and fallback "absent"
      | label   | needles |
      | present | "present", "pinned", "已配置", "nameserver-policy", "国内解析" |
      | absent  | "absent", "missing", "未配置", "fake-ip", "proxy dns" |
    When the answer "present" goes to "rule_state"
    And any other answer goes to "STOP"
    Then the primary question is "dns_policy_state"
    Then label "PIN_DOMESTIC_DNS" when the answer to "dns_policy_state" is "absent"
    And label "PROBE_DNS_POLICY_OK" otherwise

  Scenario: rule_state
    Given a choice question "rule_present" asks "Is there already a DIRECT rule for this domain in the proxy config? With DNS pinned, a DIRECT rule removes the proxy hop; with everything already in place, the remaining slowness is upstream/link quality."
      | answer  | meaning |
      | present | A DIRECT rule for the domain already exists. |
      | absent  | The domain has no DIRECT rule and may still be proxied. |
    And answer by rules on the field "ev_rule" with hit 0.9 and fallback "absent"
      | label   | needles |
      | present | "present", "exists", "已有", "direct rule" |
      | absent  | "absent", "missing", "没有", "no rule" |
    When any other answer goes to "STOP"
    Then the primary question is "rule_present"
    Then label "ADD_DIRECT_RULE" when the answer to "rule_present" is "absent"
    And label "ESCALATE_UPSTREAM" otherwise
