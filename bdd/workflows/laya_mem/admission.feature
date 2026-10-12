@decision
Feature: laya_mem_admission

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: admit
    Given a choice question "should_store" asks "Given `observation`, `exact_duplicate`, `recent_memories` and `redundancy`, should this be stored as a memory? Choose A (drop) only when there is no new recallable detail; choose B (store) when any recallable detail, correction or time-specific update is present; choose the band between only when genuinely borderline."
      | answer | meaning              |
      | A      | drop (no new recallable detail) |
      | B      | store (recallable detail present) |
    And answer "B" when the text contains "thanks", "ok", "got it", "acknowledge", "noted", "trivial", "duplicate", "fine", "acknowledged", "no need", "ack" with hit 0.1
    And a score question "future_utility" asks "How likely is this observation to be useful in a future recall? 0 = never, 1 = unlikely, 2 = likely, 3 = definitely"
      | level          |
      | 0 never                        |
      | 1 unlikely                     |
      | 2 likely                       |
      | 3 definitely                   |
    And score "future_utility" from the numeric field "future_utility" with miss 0.5
    And a score question "importance" asks "How important or impactful is this observation to the participant? 0 = trivial, 1 = minor, 2 = notable, 3 = major"
      | level          |
      | 0 trivial                      |
      | 1 minor                        |
      | 2 notable                      |
      | 3 major                        |
    And score "importance" from the numeric field "importance" with miss 0.5
    And a noul question "novelty" asks "Does `observation` add a fact absent from `recent_memories`, given the `exact_duplicate` flag?"
    And answer "B" when the text contains "new", "added", "changed", "updated" with hit 0.9
    And a noul question "redundancy" asks "Is all recallable information in `observation` already present in `recent_memories`, or is `exact_duplicate` true?"
    And answer "B" when the text contains "duplicate", "already", "same as", "repeat" with hit 0.9
    And a noul question "borderline" asks "Is `observation` weak or borderline — neither clearly drop nor clearly store? Triggers when observation hedges (`maybe`, `might`, `perhaps`, `unsure`), or when `recent_memories` already covers the topic."
    And answer "B" when the field "observation" contains "maybe", "might", "perhaps", "unsure", "borderline", "could be", "not sure", "either way", "debating", "could go" with hit 0.85 and miss 0.2
    When any other answer goes to "STOP"
    Then label "BLOCK" when the answer is "A"
    And label "CONFIRM" when "borderline" is at least 0.5
    And label "ALLOW" otherwise

