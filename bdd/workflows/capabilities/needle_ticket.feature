@decision
Feature: needle_ticket

  Background:
    Given the workflow runs at most 10 iterations

  Scenario: classify
    Given a choice question "go" asks ""
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When the answer "A" goes to "entity"
    And any other answer goes to "STOP"
    Then call the capability "cls"
      | with.text      | ${state.text} |
      | with.tool      | {"type": "function", "name": "classify_ticket", "description": "Classify a support ticket into category and urgency.", "parameters": {"type": "object", "properties": {"category": {"type": "string", "enum": ["billing", "technical", "feedback", "other"]}, "urgency": {"type": "string", "enum": ["low", "medium", "high"]}}, "required": ["category", "urgency"]}} |
      | project.category  | /arguments/category |
      | project.urgency  | /arguments/urgency |

  Scenario: entity
    Given a choice question "go" asks ""
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "ent"
      | with.text      | ${state.text} |
      | with.tool      | {"type": "function", "name": "extract_entity", "description": "Extract the customer id and order id from a support ticket.", "parameters": {"type": "object", "properties": {"customer_id": {"type": "string"}, "order_id": {"type": "string"}}, "required": ["customer_id", "order_id"]}} |
      | project.customer_id  | /arguments/customer_id |
      | project.order_id  | /arguments/order_id |

