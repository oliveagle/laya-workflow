@decision
Feature: needle_demo

  Background:
    Given the workflow runs at most 12 iterations

  Scenario: extract
    Given a choice question "go" asks ""
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When the answer "A" goes to "embed"
    And any other answer goes to "STOP"
    Then call the capability "invoice_extract"
      | with.text      | ${state.text} |
      | with.tool      | {"type": "function", "name": "invoice", "description": "Record invoice vendor, total and due date from text.", "parameters": {"type": "object", "properties": {"vendor": {"type": "string"}, "total": {"type": "number"}, "due_date": {"type": "string"}}, "required": ["vendor", "total", "due_date"]}} |
      | project.invoice  | /arguments |
      | project.confidence  | /confidence |

  Scenario: embed
    Given a choice question "go" asks ""
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When the answer "A" goes to "call"
    And any other answer goes to "STOP"
    Then call the capability "sentence_embed"
      | with.text      | turn on the kitchen lights |
      | project.embed_dim  | /dim |

  Scenario: call
    Given a choice question "go" asks ""
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "tool_call"
      | with.prompt      | what's it like in Lagos right now? |
      | with.tools      | [{"type": "function", "name": "get_weather", "description": "Get the current weather for a city.", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}] |
      | project.weather_calls  | /function_calls |
      | project.call_confidence  | /confidence |

