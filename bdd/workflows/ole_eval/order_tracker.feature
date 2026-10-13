@decision
Feature: order_tracker

  Background:
    Given the workflow runs at most 12 iterations

  Scenario: draft
    Given a choice question "event" asks "The order is a draft. Translate the utterance into exactly one typed lifecycle event; the model only proposes the event, all eligibility and money are computed locally."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_draft"
    When the answer "create" goes to "created"
    And any other answer goes to "STOP"
    Then label "create" when the answer to "event" is "create"
    And label "reject" otherwise

  Scenario: created
    Given a choice question "event" asks "The order is created and unpaid. pay records payment; cancel is allowed while unpaid. Any other event is illegal from here."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_created"
    When the answer "pay" goes to "paid"
    And the answer "cancel" goes to "cancelled_unpaid"
    And any other answer goes to "STOP"
    Then label "pay" when the answer to "event" is "pay"
    And label "cancel" when the answer to "event" is "cancel"
    And label "reject" otherwise

  Scenario: paid
    Given a choice question "event" asks "The order is paid. pack starts fulfilment; cancel is allowed and keeps the payment so a later refund is eligible."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_paid"
    When the answer "pack" goes to "fulfilling"
    And the answer "cancel" goes to "cancelled_paid"
    And any other answer goes to "STOP"
    Then label "pack" when the answer to "event" is "pack"
    And label "cancel" when the answer to "event" is "cancel"
    And label "reject" otherwise

  Scenario: fulfilling
    Given a choice question "event" asks "The order is being fulfilled. ship dispatches it; cancel is still allowed and keeps the payment."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_fulfilling"
    When the answer "ship" goes to "shipped"
    And the answer "cancel" goes to "cancelled_paid"
    And any other answer goes to "STOP"
    Then label "ship" when the answer to "event" is "ship"
    And label "cancel" when the answer to "event" is "cancel"
    And label "reject" otherwise

  Scenario: shipped
    Given a choice question "event" asks "The order has shipped. deliver is the only legal transition; cancellation is no longer possible."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_shipped"
    When the answer "deliver" goes to "delivered"
    And any other answer goes to "STOP"
    Then label "deliver" when the answer to "event" is "deliver"
    And label "reject" otherwise

  Scenario: delivered
    Given a choice question "event" asks "The order is delivered. refund is eligible (shipping is non-refundable, subtotal plus tax is returned)."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_delivered"
    When the answer "refund" goes to "refunded"
    And any other answer goes to "STOP"
    Then label "refund" when the answer to "event" is "refund"
    And label "reject" otherwise

  Scenario: cancelled_paid
    Given a choice question "event" asks "The order was cancelled after payment. refund is eligible here; anything else is illegal."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_cancelled_paid"
    When the answer "refund" goes to "refunded"
    And any other answer goes to "STOP"
    Then label "refund" when the answer to "event" is "refund"
    And label "reject" otherwise

  Scenario: cancelled_unpaid
    Given a choice question "event" asks "The order was cancelled before payment: nothing is refundable, so refund is refused. This state is terminal."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_cancelled_unpaid"
    When any other answer goes to "STOP"
    Then keep the answer to "event"

  Scenario: refunded
    Given a choice question "event" asks "The order is refunded, a terminal state. No further transition is legal."
      | answer  | meaning                                   |
      | create  | open a new order                          |
      | pay     | record that payment was received          |
      | pack    | begin fulfilment                          |
      | ship    | hand the parcel to the carrier            |
      | deliver | mark the parcel delivered                 |
      | cancel  | cancel the order                          |
      | refund  | refund a cancelled or delivered order     |
    And the answer comes from the field "ev_refunded"
    When any other answer goes to "STOP"
    Then keep the answer to "event"
