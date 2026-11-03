// Light, dark or the system's choice; remembered in localStorage and applied as
// `data-theme` on <html> (app.css defines both palettes).
export type ThemeChoice = "system" | "light" | "dark";

const media = typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;

class Theme {
  choice = $state<ThemeChoice>((localStorage.getItem("snapreq.theme") as ThemeChoice | null) ?? "system");
  systemDark = $state(media?.matches ?? false);

  get resolved(): "light" | "dark" {
    return this.choice === "system" ? (this.systemDark ? "dark" : "light") : this.choice;
  }

  set(choice: ThemeChoice) {
    this.choice = choice;
    localStorage.setItem("snapreq.theme", choice);
    this.apply();
  }

  apply() {
    document.documentElement.dataset.theme = this.resolved;
  }

  constructor() {
    media?.addEventListener("change", (e) => {
      this.systemDark = e.matches;
      this.apply();
    });
  }
}

export const theme = new Theme();
