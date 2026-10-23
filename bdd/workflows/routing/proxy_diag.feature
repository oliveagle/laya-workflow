@decision
Feature: proxy_diag
  Diagnose why a LAN client behind an OpenClash fake-ip bypass router cannot
  reach a blocked site (e.g. Chrome cannot open YouTube). It is a first-fail
  walk over four probes, in the order a real diagnosis runs them:

    1. dns_probe      -- what the client's resolver returned for the blocked
                         domain: fake-ip (198.18.0.0/16) / poisoned / real-ip /
                         no answer.
    2. gateway_probe  -- is the client default gateway the OpenClash box itself?
                         fake-ip only gets intercepted when traffic traverses it.
    3. intercept_probe-- does the clash log show this client's connections?
                         (proof the nft redirect/tproxy actually applied).
    4. proxy_probe    -- end-to-end via the proxy port: reachable, auth, or
                         upstream node dead.

  The harness (the agent doing the diagnosis) fills four evidence fields before
  each call; the graph reads nothing else:

    - ev_dns       what the client's resolver returned for the blocked domain,
                   as a classification plus the raw answer, e.g.
                   "fake-ip 198.18.0.23", "poisoned 104.244.42.197",
                   "real-ip 142.250.x.x" or "timeout". Distinguishing GFW
                   poisoning from a plain real IP needs the diagnosing agent's
                   judgement, so it carries its own label; the raw IP alone
                   falls back to real_ip (the safe reading for a non-fake-ip
                   answer).
    - ev_gateway   the client's default gateway, e.g. "192.168.2.1 main router"
                   or "192.168.2.249 openclash box".
    - ev_intercept whether /tmp/openclash.log shows this client IP at all,
                   e.g. "no client traffic seen" or "saw 192.168.2.83 youtube".
    - ev_proxy     the curl result through the proxy port, e.g. "200 ok",
                   "407 auth required", "refused", "node timeout".

  The first probe that fails ends the walk and names the fix:
  FIX_DNS / FIX_GATEWAY / FIX_INTERCEPT / FIX_PROXY_AUTH / FIX_PROXY_PORT /
  FIX_NODE; a run that clears every probe reaches ALL_GOOD.

  Background:
    Given the workflow runs at most 4 iterations

  Scenario: dns_probe
    Given a choice question "dns_state" asks "What did the client's resolver return for the blocked domain? fake-ip means OpenClash's DNS took over; poisoned or real-ip means it did not; no answer means DNS itself is broken."
      | answer    | meaning |
      | fake_ip   | A 198.18.0.0/16 fake-ip address: OpenClash DNS is serving this client. |
      | poisoned  | A wrong real-looking IP (GFW DNS poisoning): DNS bypassed OpenClash. |
      | real_ip   | The real blocked-site IP: DNS bypassed OpenClash entirely. |
      | no_answer | No response or timeout: DNS is broken for this client. |
    And answer by rules on the field "ev_dns" with hit 0.9 and fallback "real_ip"
      | label     | needles |
      | fake_ip   | "198.18", "fake-ip", "fake_ip", "fake ip", "fakeip" |
      | poisoned  | "poison", "poisoned", "污染", "wrong ip", "gfw" |
      | real_ip   | "real", "real-ip", "real_ip", "real ip", "bypass" |
      | no_answer | "timeout", "no answer", "no response", "无响应", "超时" |
    When the answer "fake_ip" goes to "gateway_probe"
    And any other answer goes to "STOP"
    Then the primary question is "dns_state"
    Then label "FIX_DNS" when the answer to "dns_state" is one of "poisoned", "real_ip", "no_answer"
    And label "PROBE_DNS_OK" otherwise

  Scenario: gateway_probe
    Given a choice question "gateway_state" asks "Is the client's default gateway the OpenClash bypass router itself? fake-ip addresses are only intercepted by OpenClash's nftables when traffic routes through that box; a gateway pointing at the main router sends fake-ip packets straight into the internet blackhole."
      | answer     | meaning |
      | openclash  | The default gateway is the OpenClash box, so fake-ip can be intercepted. |
      | main_router | The default gateway is the main/ISP router: fake-ip traffic bypasses OpenClash. The single most common fake-ip misconfiguration. |
      | unknown    | The gateway cannot be determined from the evidence. |
    And answer by rules on the field "ev_gateway" with hit 0.9 and fallback "unknown"
      | label      | needles |
      | openclash  | "openclash", "旁路由", "249" |
      | main_router | "main router", "主路由", "192.168.2.1", "isp router" |
    When the answer "openclash" goes to "intercept_probe"
    And any other answer goes to "STOP"
    Then the primary question is "gateway_state"
    Then label "FIX_GATEWAY" when the answer to "gateway_state" is "main_router"
    And label "FIX_GATEWAY" when the answer to "gateway_state" is "unknown"
    And label "PROBE_GATEWAY_OK" otherwise

  Scenario: intercept_probe
    Given a choice question "intercept_state" asks "Does the OpenClash log show any connection from this client IP? fake-ip being returned plus a correct gateway should produce visible client connections; their absence means the nft redirect/tproxy rules are not applying."
      | answer | meaning |
      | yes    | The clash log lists this client IP (traffic is being intercepted). |
      | no     | The clash log only shows the router's own IP: interception is not happening. |
    And answer by rules on the field "ev_intercept" with hit 0.9 and fallback "no"
      | label | needles |
      | yes   | "yes", "saw", "出现", "matched" |
      | no    | "no", "未见", "empty", "only router", "没有" |
    When the answer "yes" goes to "proxy_probe"
    And any other answer goes to "STOP"
    Then the primary question is "intercept_state"
    Then label "FIX_INTERCEPT" when the answer to "intercept_state" is "no"
    And label "PROBE_INTERCEPT_OK" otherwise

  Scenario: proxy_probe
    Given a choice question "proxy_state" asks "End-to-end result of fetching the blocked site through the proxy port? Distinguishes a healthy proxy, an auth requirement on the port, a dead port, and a dead upstream node."
      | answer        | meaning |
      | ok            | The proxy port served the blocked site (HTTP 200): the whole chain works. |
      | auth_required | The port answers but demands authentication (407). |
      | port_down     | The port refuses connections or times out. |
      | node_down     | The port works but the upstream node fails the fetch. |
    And answer by rules on the field "ev_proxy" with hit 0.9 and fallback "port_down"
      | label         | needles |
      | ok            | "200", "ok", "通" |
      | auth_required | "407", "auth", "认证" |
      | node_down     | "node", "节点", "upstream", "dial error" |
      | port_down     | "refused", "timeout", "reject", "reject" |
    When any other answer goes to "STOP"
    Then the primary question is "proxy_state"
    Then label "FIX_PROXY_AUTH" when the answer to "proxy_state" is "auth_required"
    And label "FIX_PROXY_PORT" when the answer to "proxy_state" is "port_down"
    And label "FIX_NODE" when the answer to "proxy_state" is "node_down"
    And label "ALL_GOOD" otherwise
