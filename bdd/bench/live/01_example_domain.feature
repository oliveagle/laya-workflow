Feature: example_domain live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_attr)
  Scenario: example_domain reads the minimal static page
    Given I am on "https://example.com/"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 5" is true
    When I extract the page title into title
    Then the saved value "title" equals text "Example Domain"
    When I extract the page url into url
    Then the saved value "url" contains text "example.com"
    Then the page url contains "example.com"
    When I extract the text of the element "p" into bodytext
    Then the saved value "bodytext" contains text "documentation examples"
    Then javascript "document.body.innerText.length > 40" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('p').length >= 1" is true
    When I extract the attribute "class" of the element "p" into first_attr
    Then javascript "document.querySelectorAll('p').length >= 1" is true
    When I navigate to "https://example.com/?live=1"
    When I wait for the element "body"
    Then javascript "location.search.indexOf('live=1') !== -1" is true
    Then the page title contains "Example"
    When I extract the page title into title2
    Then the saved value "title2" contains text "Example"
    Then the page url contains "example.com"
    Then the saved value "title" contains text "Example Domain"
