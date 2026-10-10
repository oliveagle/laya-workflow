Feature: wikipedia_rust live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_href, title2)
  Scenario: wikipedia_rust reads en.wikipedia.org on the real web
    Given I am on "https://en.wikipedia.org/wiki/Rust_(programming_language)"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 10" is true
    When I extract the page title into title
    Then the saved value "title" contains text "Rust (programming language)"
    When I extract the page url into url
    Then the saved value "url" contains text "en.wikipedia.org"
    Then the page url contains "Rust_(programming_language)"
    When I extract the text of the element "body" into bodytext
    Then javascript "document.body.innerText.length > 40" is true
    Then javascript "document.querySelectorAll('#mw-content-text, .mw-parser-output, #content').length >= 1" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    When I extract the attribute "href" of the element "a[href]" into first_href
    Then javascript "document.querySelector('a[href]').getAttribute('href').length >= 1" is true
    When I navigate to "https://en.wikipedia.org/wiki/Systems_programming"
    When I wait for the element "body"
    Then javascript "location.href.indexOf('Systems_programming') !== -1" is true
    Then the page title contains "Systems programming"
    When I extract the page title into title2
    Then the saved value "title2" contains text "Systems programming"
    Then the page url contains "en.wikipedia.org"
