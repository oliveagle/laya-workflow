Feature: hackernews live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_href, title2)
  Scenario: hackernews reads news.ycombinator.com on the real web
    Given I am on "https://news.ycombinator.com/"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 10" is true
    When I extract the page title into title
    Then the saved value "title" contains text "Hacker News"
    When I extract the page url into url
    Then the saved value "url" contains text "news.ycombinator.com"
    Then the page url contains "news.ycombinator.com"
    When I extract the text of the element "body" into bodytext
    Then javascript "document.body.innerText.length > 40" is true
    Then javascript "document.querySelectorAll('.athing, .itemlist, table').length >= 1" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    When I extract the attribute "href" of the element "a[href]" into first_href
    Then javascript "document.querySelector('a[href]').getAttribute('href').length >= 1" is true
    When I navigate to "https://news.ycombinator.com/newest"
    When I wait for the element "body"
    Then javascript "location.href.indexOf('newest') !== -1" is true
    Then the page title contains "Hacker News"
    When I extract the page title into title2
    Then the saved value "title2" contains text "Hacker News"
    Then the page url contains "news.ycombinator.com"
