@decision
Feature: access_path_diag
  Diagnose why a domestic service is slow to reach (the Feishu-docs case).
  It is a first-fail walk over four probes, in the order a real diagnosis runs
  them:

    1. dns_split  -- does the system resolver (:53) return the same answer as a
                     DoH resolver bypassing the :53 path (AliDNS 223.5.5.5,
                     doh.pub)? A mismatch means a local DNS proxy / bypass
                     router is choosing the edge, not the real DNS.
    2. edge_geo   -- is the resolved CDN edge domestic? A domestic service
                     served from an overseas edge is pure added RTT.
    3. proxy_path -- does this domain's traffic traverse a proxy (system proxy,
                     TUN, or a bypass-router rule)? A domestic domain behind a
                     proxy pays the proxy chain twice.
    4. rtt_budget -- are TLS handshake and TTFB within budget
                     (TLS < 300ms, TTFB < 500ms)?

  The harness fills four evidence fields; the graph reads nothing else. Each
  value must START with its canonical token (same / differs / domestic /
  overseas / direct / proxied / fast / slow); detail may follow but must not
  repeat another answer's token:

    - ev_dns_diff  "same" (system :53 == DoH) or "differs <detail>"
                   (e.g. "differs :53 139.177.246.207 (HK) vs DoH 183.192.176.85").
    - ev_edge      "domestic 183.192.176.85", "overseas 139.177.246.207 (HK)",
                   "overseas 155.102.209.202 (JP)" or "unknown".
    - ev_proxy     "direct", "proxied (system proxy)", "proxied (TUN)",
                   "proxied (gateway rule)" or "unknown".
    - ev_rtt       "fast tls=31ms ttfb=93ms" or "slow tls=878ms ttfb=1352ms"
                   (the leading word is the harness's budget verdict; detail
                   after it is free-form).

  Calibration (measured 2026-10-24 on the Feishu case):
    domestic edge 183.192.176.85  TLS 25-32ms  TTFB 93-155ms   -> healthy
    HK edge       139.177.246.207 TLS 388-878ms TTFB 644-1352ms -> 5-10x slower
    :53 hijack returned HK/Tokyo edges for every resolver tested
    (223.5.5.5 / 119.29.29.29 / 180.76.76.76 / 8.8.8.8), while DoH returned
    domestic edges (183.192.176.85 / 36.156.72.x / 223.111.230.x).

  Emitted labels: FIX_DNS_SPLIT, FIX_EDGE_OVERSEAS, FIX_PROXY_RULE,
  FIX_ROUTE_QUALITY, NETWORK_OK.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: dns_split
    Given a choice question "dns_state" asks "Does the system :53 resolver return the same answer as DoH (dig @223.5.5.5 vs https://doh.pub/dns-query)? A mismatch means a local DNS proxy (bypass router / OpenClash / fake-ip) is choosing the edge."
      | answer  | meaning |
      | same    | :53 and DoH agree: no DNS interception for this domain. |
      | differs | :53 and DoH disagree: a local resolver is answering instead of real DNS. |
    And answer by rules on the field "ev_dns_diff" with hit 0.9 and fallback "same"
      | label   | needles |
      | differs | "differs", "differ", "mismatch", "不一致", "vs doh" |
      | same    | "same", "match", "一致" |
    When the answer "same" goes to "edge_geo"
    And any other answer goes to "STOP"
    Then the primary question is "dns_state"
    Then label "FIX_DNS_SPLIT" when the answer to "dns_state" is "differs"
    And label "PROBE_DNS_OK" otherwise

  Scenario: edge_geo
    Given a choice question "edge_state" asks "Is the resolved CDN edge domestic? A domestic service (feishu.cn, feishucdn.com) served from an overseas edge pays the cross-border RTT on every request."
      | answer   | meaning |
      | domestic | The edge IP is in mainland China. |
      | overseas | The edge IP is outside mainland China (HK/Tokyo/US...). |
      | unknown  | The edge location cannot be determined. |
    And answer by rules on the field "ev_edge" with hit 0.9 and fallback "unknown"
      | label    | needles |
      | domestic | "domestic", "境内", "mainland", "cn", "上海", "北京", "广州" |
      | overseas | "overseas", "海外", "hk", "hong kong", "香港", "tokyo", "东京", "jp", "us" |
    When the answer "domestic" goes to "proxy_path"
    And any other answer goes to "STOP"
    Then the primary question is "edge_state"
    Then label "FIX_EDGE_OVERSEAS" when the answer to "edge_state" is one of "overseas", "unknown"
    And label "PROBE_EDGE_OK" otherwise

  Scenario: proxy_path
    Given a choice question "proxy_state" asks "Does this domain's traffic traverse a proxy (system proxy, TUN, or a bypass-router rule)? A domestic domain sent through a proxy chain is slower than direct, and the proxy's DNS can also pick a foreign edge."
      | answer  | meaning |
      | direct  | Traffic goes direct to the domestic edge. |
      | proxied | Traffic goes through a proxy/TUN/bypass-router rule. |
      | unknown | The path cannot be determined from the evidence. |
    And answer by rules on the field "ev_proxy" with hit 0.9 and fallback "unknown"
      | label   | needles |
      | direct  | "direct", "直连" |
      | proxied | "proxy", "proxied", "代理", "tun", "gateway rule", "旁路由" |
    When the answer "direct" goes to "rtt_budget"
    And any other answer goes to "STOP"
    Then the primary question is "proxy_state"
    Then label "FIX_PROXY_RULE" when the answer to "proxy_state" is one of "proxied", "unknown"
    And label "PROBE_PROXY_OK" otherwise

  Scenario: rtt_budget
    Given a choice question "rtt_state" asks "Are TLS handshake and TTFB within budget (TLS < 300ms, TTFB < 500ms on a domestic edge)? Above budget with a clean path points at link quality / MTU / congestion rather than DNS or proxy."
      | answer | meaning |
      | fast   | TLS < 300ms and TTFB < 500ms: the path is healthy. |
      | slow   | TLS >= 300ms or TTFB >= 500ms: the path itself is slow. |
    And answer by rules on the field "ev_rtt" with hit 0.9 and fallback "slow"
      | label | needles |
      | fast  | "fast", "预算内", "ok" |
      | slow  | "slow", "超预算", "慢", "timeout", "超时" |
    When any other answer goes to "STOP"
    Then the primary question is "rtt_state"
    Then label "NETWORK_OK" when the answer to "rtt_state" is "fast"
    And label "FIX_ROUTE_QUALITY" otherwise
