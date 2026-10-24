@decision
Feature: dns_check
  Machine DNS check: confirm the resolver actually in effect, resolve a known
  baseline domain through it, judge whether the answer is the expected address
  (vs poisoned / fake-ip), then probe the candidate public DNS servers directly
  and name the conclusion. It is a first-fail walk over four probes, in the
  order a real check runs them:

    1. resolver_probe -- is any DNS server configured on this machine?
                         (macOS `scutil --dns`, Linux `resolvectl status`).
    2. resolve_probe  -- does the machine's own resolver return an answer for
                         the baseline domain (`dig <name>`, `nslookup <name>`)?
    3. answer_probe   -- is that answer the expected real address, or is it
                         poisoned (GFW-style) / a 198.18.0.0/16 fake-ip?
    4. public_probe   -- query the candidate public DNS servers directly
                         (`dig @<ip> <name> +short`): which ones answer right?

  The harness (the agent doing the check) fills four evidence fields; the graph
  reads nothing else. Every evidence value must START with its canonical token
  (present / ok / expected / all_ok ...); free-form detail may follow, but must
  not repeat another answer's token (e.g. an "expected ..." detail on a
  "poisoned ..." line would be misread):

    - ev_resolver  the machine's DNS configuration, e.g.
                   "nameserver 192.168.2.1", "dns servers 10.126.126.6",
                   "none" or "empty".
    - ev_resolve   the machine resolver's answer, e.g. "ok 93.184.216.34",
                   "timeout", "servfail", "nxdomain".
    - ev_answer    the quality of that answer, e.g.
                   "expected 93.184.216.34", "poisoned 1.2.3.4",
                   "fake-ip 198.18.0.5".
    - ev_public    the direct public-DNS probe results, e.g.
                   "all ok", "partial 223.5.5.5,119.29.29.29", "none ok".

  Public DNS catalog (probe with `dig @<ip> <name> +short`):
    AliDNS      223.5.5.5 / 223.6.6.6        (ECS-aware, good domestic reach)
    DNSPod      119.29.29.29 / 182.254.116.116 (fast, multi-ISP)
    114DNS      114.114.114.114 / 114.114.115.115
                (security 114.114.114.119, family 114.114.114.110)
    Baidu DNS   180.76.76.76
    Google DNS  8.8.8.8 / 8.8.4.4

  Emitted labels: FIX_NO_RESOLVER, FIX_RESOLVER, FIX_NXDOMAIN, FIX_POISONED,
  SUGGEST_PUBLIC_DNS, FIX_UPSTREAM, DNS_OK.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: resolver_probe
    Given a choice question "resolver_state" asks "Does this machine have any DNS server configured? macOS: scutil --dns; Linux: resolvectl status. 'absent' means the system config lists no nameserver at all."
      | answer  | meaning |
      | present | At least one nameserver/resolver is configured in the system DNS settings. |
      | absent  | No DNS server appears anywhere in the system configuration. |
    And answer by rules on the field "ev_resolver" with hit 0.9 and fallback "present"
      | label  | needles |
      | absent | "none", "empty", "no dns", "未配置", "空" |
    When the answer "present" goes to "resolve_probe"
    And any other answer goes to "STOP"
    Then the primary question is "resolver_state"
    Then label "FIX_NO_RESOLVER" when the answer to "resolver_state" is "absent"
    And label "PROBE_RESOLVER_OK" otherwise

  Scenario: resolve_probe
    Given a choice question "resolve_state" asks "Does the machine's own resolver return an answer for the baseline domain? ok = an A/AAAA answer; timeout = no response; servfail = the server answered with a failure; nxdomain = the server says the name does not exist."
      | answer   | meaning |
      | ok       | The resolver returned an address. |
      | timeout  | No response before the query deadline. |
      | servfail | The resolver answered SERVFAIL / refused. |
      | nxdomain | The resolver answered NXDOMAIN (the name itself does not exist). |
    And answer by rules on the field "ev_resolve" with hit 0.9 and fallback "timeout"
      | label    | needles |
      | ok       | "ok", "answer", "成功", "resolved" |
      | timeout  | "timeout", "超时", "no response", "无响应" |
      | servfail | "servfail", "refused", "失败", "error" |
      | nxdomain | "nxdomain", "no such", "不存在", "解析不到" |
    When the answer "ok" goes to "answer_probe"
    And any other answer goes to "STOP"
    Then the primary question is "resolve_state"
    Then label "FIX_RESOLVER" when the answer to "resolve_state" is one of "timeout", "servfail"
    And label "FIX_NXDOMAIN" when the answer to "resolve_state" is "nxdomain"
    And label "PROBE_RESOLVE_OK" otherwise

  Scenario: answer_probe
    Given a choice question "answer_state" asks "Is the resolver's address the expected real one? expected = matches the known-good address; poisoned = a wrong real-looking address (DNS hijacking); fake_ip = a 198.18.0.0/16 address, i.e. a local resolver (OpenClash & co.) took the name over."
      | answer   | meaning |
      | expected | The answer matches the known-good address. |
      | poisoned | A wrong real-looking address: DNS hijacking / poisoning. |
      | fake_ip  | A 198.18.0.0/16 fake-ip: a local DNS proxy answered instead of upstream. |
    And answer by rules on the field "ev_answer" with hit 0.9 and fallback "expected"
      | label    | needles |
      | poisoned | "poisoned", "poison", "污染", "wrong ip", "gfw", "hijacked", "hijack", "劫持" |
      | fake_ip  | "198.18", "fake-ip", "fake_ip", "fake ip", "fakeip" |
    When the answer "fake_ip" goes to "public_probe"
    And the answer "poisoned" goes to "public_probe"
    And any other answer goes to "STOP"
    Then the primary question is "answer_state"
    Then label "DNS_OK" when the answer to "answer_state" is "expected"
    And label "PROBE_ANSWER_OK" otherwise

  Scenario: public_probe
    Given a choice question "public_state" asks "Querying the candidate public DNS servers directly (dig @<ip> <name> +short), which of them return the expected address? all_ok = every one; partial = at least one; none_ok = none of them."
      | answer   | meaning |
      | all_ok   | Every candidate public DNS returned the expected address. |
      | partial  | Some candidates returned the expected address; at least one failed. |
      | none_ok  | No candidate public DNS returned the expected address. |
    And answer by rules on the field "ev_public" with hit 0.9 and fallback "none_ok"
      | label   | needles |
      | all_ok  | "all ok", "all_ok", "全部", "every" |
      | partial | "partial", "部分", "some" |
      | none_ok | "none", "none ok", "全失败", "all fail" |
    When any other answer goes to "STOP"
    Then the primary question is "public_state"
    Then label "SUGGEST_PUBLIC_DNS" when the answer to "public_state" is one of "all_ok", "partial"
    And label "FIX_UPSTREAM" otherwise
