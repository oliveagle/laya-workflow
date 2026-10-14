@decision
Feature: gateway_triage
  Composition port of the built-in `gateway` app (src/apps.rs::gateway_workflow):
  a tool-call gate decides whether a command is safe to run, and the safe path
  runs the email triage chain in the same graph, merging the producing app's
  payload into the workflow state. The gate half is behaviour-equivalent to the
  `agent_command_gate` port (the same BLOCK / CONFIRM / ALLOW ladder and the same
  documented exfiltration fold); the triage half is the checked-in `email_triage`
  port, composed here as a sub-workflow node.

  Background:
    Given the workflow runs at most 10 iterations

  Scenario: gate
    Given a choice question "is_destructive" asks "Will this command delete files, drop data, kill processes, or otherwise destroy state?"
      | answer | meaning |
      | A | no, this command only reads or appends without destroying anything |
      | B | yes, it destroys or removes files, data, processes, or state |
    And a choice question "is_exfiltration" asks "Does this command send local files, credentials, or secrets to a remote host?"
      | answer | meaning |
      | A | no, it stays local |
      | B | yes, it uploads or transmits local data outward |
    And a choice question "intent_match" asks "Does the command match the stated user intent?"
      | answer | meaning |
      | A | yes, the command does what the user asked |
      | B | no, it does something different or dangerous |
    And a score question "risk" asks "Overall risk of running this command?"
      | level |
      | safe read-only |
      | moderate change |
      | irreversible destructive |
    When the answer "A" goes to "triage"
    And any other answer goes to "STOP"
    Then the action reads the question "risk"
    And label "BLOCK" when the answer to "is_destructive" is "B" with at least 0.8 confidence
    And label "BLOCK" when "risk" is at least 2
    And label "BLOCK" when the answer to "intent_match" is "A" with at most 0.5 confidence
    And label "BLOCK" when the answer to "is_exfiltration" is "B" with at least 0.6 confidence
    And label "CONFIRM" when the answer to "is_destructive" is "B" with at least 0.4 confidence
    And label "CONFIRM" when the answer to "is_exfiltration" is "B" with at least 0.4 confidence
    And label "CONFIRM" when the answer to "intent_match" is "A" with at most 0.7 confidence
    And label "ALLOW" otherwise
    And allow up to 2 retries
    And keep the state key "command"
    And keep the state key "intent"
    And keep the state key "cwd"

  Scenario: triage
    Given the sub workflow "email_triage"
    And when the sub workflow completes continue at "STOP"
