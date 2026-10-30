"use strict";
for (const demo of document.querySelectorAll("[data-reader-theme]")) {
  const themes = demo.querySelector(".themes");
  if (!themes) continue;
  themes.hidden = false;
  themes.addEventListener("click", (event) => {
    const button = event.target.closest("button[data-theme]");
    if (!button || !themes.contains(button)) return;
    demo.dataset.readerTheme = button.dataset.theme;
    for (const control of themes.querySelectorAll("button")) {
      control.setAttribute("aria-pressed", String(control === button));
    }
  });
}
