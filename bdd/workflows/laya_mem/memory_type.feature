@decision
Feature: laya_mem_memory_type

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: type
    Given a noul question "episodic" asks "Does `observation` describe a particular experience or event involving a participant?"
      | truthy | A specific past, current or planned event, even if its exact time is unstated. |
      | falsy  | Only a general fact, procedure or preference with no particular event. |
    And answer "B" when the field "observation" contains "planted", "started", "presented", "visited", "met", "began", "launched", "happened", "on friday", "went", "joined", "got a", "deploy" with hit 0.8 and miss 0.1
    And a noul question "semantic" asks "Does `observation` state a fact about a person, entity or the world that remains useful beyond this conversational turn?"
      | truthy | An attributable fact or relationship, even when it also appears in an episodic account. |
      | falsy  | Only a transient conversational acknowledgement or a question with no asserted fact. |
    And answer "B" when the field "observation" contains "is a", "lives in", "works at", "born in", "facts about", "located in", "hometown", "capital", "was released" with hit 0.8 and miss 0.1
    And a noul question "procedural" asks "Does `observation` describe how to carry out a task?"
      | truthy | An instruction, ordered step, method or actionable rule for performing a task. |
      | falsy  | Merely mentions doing a task without describing how. |
    And answer "B" when the field "observation" contains "how to", "steps", "recipe", "instructions", "tutorial", "guide", "procedure", "first", "then", "click", "follow" with hit 0.8 and miss 0.1
    And a noul question "preference" asks "Does `observation` express a participant's preference, aversion or habitual choice?"
      | truthy | An attributable like, dislike, preferred option or habitual choice. |
      | falsy  | An isolated action alone, another person's unattributed preference, or no preference evidence. |
    And answer "B" when the field "observation" contains "prefer", "prefers", "like", "likes", "favorite", "preference", "wants a weekly", "would rather", "always orders", "enjoys", "dislikes" with hit 0.9
    When any other answer goes to "classify"
    Then keep the answer to "episodic"
    And keep the answer to "semantic"
    And keep the answer to "procedural"
    And keep the answer to "preference"

  Scenario: classify
    Given a noul question "episodic" asks "Does `observation` describe a particular experience or event involving a participant?"
      | truthy | A specific past, current or planned event, even if its exact time is unstated. |
      | falsy  | Only a general fact, procedure or preference with no particular event. |
    And answer "B" when the field "observation" contains "planted", "started", "presented", "visited", "met", "began", "launched", "happened", "on friday", "went", "joined", "got a", "deploy" with hit 0.8 and miss 0.1
    And a noul question "semantic" asks "Does `observation` state a fact about a person, entity or the world that remains useful beyond this conversational turn?"
      | truthy | An attributable fact or relationship, even when it also appears in an episodic account. |
      | falsy  | Only a transient conversational acknowledgement or a question with no asserted fact. |
    And answer "B" when the field "observation" contains "is a", "lives in", "works at", "born in", "facts about", "located in", "hometown", "capital", "was released" with hit 0.8 and miss 0.1
    And a noul question "procedural" asks "Does `observation` describe how to carry out a task?"
      | truthy | An instruction, ordered step, method or actionable rule for performing a task. |
      | falsy  | Merely mentions doing a task without describing how. |
    And answer "B" when the field "observation" contains "how to", "steps", "recipe", "instructions", "tutorial", "guide", "procedure", "first", "then", "click", "follow" with hit 0.8 and miss 0.1
    And a noul question "preference" asks "Does `observation` express a participant's preference, aversion or habitual choice?"
      | truthy | An attributable like, dislike, preferred option or habitual choice. |
      | falsy  | An isolated action alone, another person's unattributed preference, or no preference evidence. |
    And answer "B" when the field "observation" contains "prefer", "prefers", "like", "likes", "favorite", "preference", "wants a weekly", "would rather", "always orders", "enjoys", "dislikes" with hit 0.9
    When any other answer goes to "STOP"
    Then label "TYPE_EPISODIC" when "episodic" is at least 0.5
    And label "TYPE_SEMANTIC" when "semantic" is at least 0.5
    And label "TYPE_PROCEDURAL" when "procedural" is at least 0.5
    And label "TYPE_PREFERENCE" when "preference" is at least 0.5
    And label "TYPE_OTHER" otherwise

