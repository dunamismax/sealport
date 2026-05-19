(function () {
  "use strict";

  var STORAGE_KEY = "fileferry-theme";
  var root = document.documentElement;

  function apply(theme) {
    root.setAttribute("data-theme", theme);
    var meta = document.querySelector('meta[name="theme-color"]:not([media])');
    if (meta) {
      meta.setAttribute("content", theme === "light" ? "#ffffff" : "#0d1117");
    }
  }

  function current() {
    var t = root.getAttribute("data-theme");
    return t === "light" ? "light" : "dark";
  }

  function toggle() {
    var next = current() === "dark" ? "light" : "dark";
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch (e) {
      /* ignore storage errors */
    }
    apply(next);
  }

  var buttons = document.querySelectorAll("[data-theme-toggle]");
  for (var i = 0; i < buttons.length; i++) {
    buttons[i].addEventListener("click", toggle);
  }

  // Track system preference changes only when the user has not made an explicit choice.
  if (window.matchMedia) {
    var mql = window.matchMedia("(prefers-color-scheme: light)");
    var handler = function (e) {
      try {
        if (localStorage.getItem(STORAGE_KEY)) return;
      } catch (err) {
        /* ignore */
      }
      apply(e.matches ? "light" : "dark");
    };
    if (mql.addEventListener) {
      mql.addEventListener("change", handler);
    } else if (mql.addListener) {
      mql.addListener(handler);
    }
  }
})();
