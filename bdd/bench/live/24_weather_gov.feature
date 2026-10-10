Feature: weather_gov live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_href, title2)
  Scenario: weather_gov reads www.weather.gov on the real web
    Given I am on "https://www.weather.gov/"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 10" is true
    When I extract the page title into title
    Then the saved value "title" contains text "National Weather Service"
    When I extract the page url into url
    Then the saved value "url" contains text "www.weather.gov"
    Then the page url contains "weather.gov"
    When I extract the text of the element "body" into bodytext
    Then javascript "document.body.innerText.length > 40" is true
    Then javascript "document.querySelectorAll('div, nav, main').length >= 1" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    When I extract the attribute "href" of the element "a[href]" into first_href
    Then javascript "document.querySelector('a[href]').getAttribute('href').length >= 1" is true
    When I navigate to "https://www.weather.gov/"
    When I wait for the element "body"
    Then javascript "location.href.indexOf('weather.gov') !== -1" is true
    Then the page title contains "National Weather Service"
    When I extract the page title into title2
    Then the saved value "title2" contains text "National Weather Service"
    Then the page url contains "www.weather.gov"
