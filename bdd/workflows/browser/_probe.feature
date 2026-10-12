# Live-fire probe: does the generic BDD workflow vocabulary drive a real site?
Feature: Wiki probe
  Background:
    Given the browser is ready

  @outputs(title, url, doc_title, first_href, title2, url2)
  Scenario: Read a real encyclopedia page and its neighbour
    Given I am on "https://en.wikipedia.org/wiki/Rust_(programming_language)"
    When I wait for the element "body"
    When I extract the page title into title
    When I extract the page url into url
    When I extract the text of the element "title" into doc_title
    When I extract the attribute "href" of the element "a[href^=\"http\"]" into first_href
    When I click the element "#consent-banner-absent" if it is present
    When I press the key "Escape"
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    Then javascript "document.querySelectorAll('h1,h2,h3,h4').length >= 1" is true
    Then the page title contains "Rust"
    Then the saved value "doc_title" contains text "Rust"
    Then the saved value "first_href" contains text "http"
    Then the element "body" is visible
    When I navigate to "https://en.wikipedia.org/wiki/Systems_programming"
    When I wait for the element "body"
    When I extract the page title into title2
    When I extract the page url into url2
    Then javascript "location.href.indexOf('Systems_programming') !== -1" is true
    Then the saved value "title2" contains text "Systems programming"
    Then the page url contains "Systems_programming"
    Then the saved value "url" contains text "wikipedia.org"
