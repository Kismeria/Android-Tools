"use strict";
/* Theme catalog. A theme is a design (data-style on <html>: shapes, fonts, borders) plus a
   palette (data-palette: colors). Designs marked `fixed` bring their own colors. */

const STYLES = [
  { id: "modern", name: "Modern", note: "OLED", palette: "oled" },
  { id: "cyberpunk", name: "Cyberpunk", note: "2077", fixed: true },
  { id: "win95", name: "Windows 95", note: "1995", fixed: true },
  { id: "xp", name: "Windows XP", note: "Luna", fixed: true },
  { id: "aqua", name: "Aqua", note: "Mac OS X", fixed: true },
  { id: "terminal", name: "Terminal", note: "CRT", palette: "hackerman" },
  { id: "brutal", name: "Brutal", note: "Neo", palette: "sunny" },
  { id: "glass", name: "Glass", note: "Aero", palette: "tokyo-night" },
  { id: "material", name: "Material You", note: "Android", palette: "m3" },
  { id: "synthwave", name: "Synthwave", note: "1984", fixed: true },
  { id: "gameboy", name: "Game Boy", note: "DMG", fixed: true },
  { id: "paper", name: "Paper", note: "Sketch", fixed: true },
];

// Palettes after Omarchy's themes, plus a few classics.
const PALETTES = [
  { id: "oled", name: "OLED Black" },
  { id: "tokyo-night", name: "Tokyo Night" },
  { id: "catppuccin", name: "Catppuccin" },
  { id: "catppuccin-latte", name: "Catppuccin Latte" },
  { id: "everforest", name: "Everforest" },
  { id: "gruvbox", name: "Gruvbox" },
  { id: "kanagawa", name: "Kanagawa" },
  { id: "nord", name: "Nord" },
  { id: "matte-black", name: "Matte Black" },
  { id: "osaka-jade", name: "Osaka Jade" },
  { id: "ristretto", name: "Ristretto" },
  { id: "rose-pine", name: "Rosé Pine" },
  { id: "flexoki-light", name: "Flexoki Light" },
  { id: "ethereal", name: "Ethereal" },
  { id: "hackerman", name: "Hackerman" },
  { id: "dracula", name: "Dracula" },
  { id: "m3", name: "Material Light" },
  { id: "m3-dark", name: "Material Dark" },
  { id: "sunny", name: "Sunny" },
];

const ACCENTS = ["#3ddc84", "#4aa8ff", "#7aa2f7", "#b16cff", "#ff5ca8", "#ff5c5c", "#ff8a3d", "#ffc233", "#2dd4bf"];

const Theme = (() => {
  const root = document.documentElement;
  const style = (id) => STYLES.find((s) => s.id === id) || STYLES[0];

  const hexToRgb = (hex) => {
    const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
    if (!m) return null;
    const v = parseInt(m[1], 16);
    return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
  };
  const luminance = (hex) => {
    const rgb = hexToRgb(hex);
    if (!rgb) return 0;
    const [r, g, b] = rgb.map((c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };

  function apply(t) {
    const s = style(t.style);
    root.dataset.style = s.id;
    root.dataset.palette = s.fixed ? "none" : (PALETTES.some((p) => p.id === t.palette) ? t.palette : s.palette);
    for (const k of ["--accent", "--accent-hi", "--accent-dim", "--on-accent"]) root.style.removeProperty(k);
    if (t.accent && !s.fixed) {
      root.style.setProperty("--accent", t.accent);
      root.style.setProperty("--accent-hi", `color-mix(in srgb, ${t.accent} 80%, #fff)`);
      root.style.setProperty("--accent-dim", `color-mix(in srgb, ${t.accent} 22%, var(--bg))`);
      root.style.setProperty("--on-accent", luminance(t.accent) > 0.4 ? "#000" : "#fff");
    }
  }

  /** Colors for the native title bar, read from the applied tokens. */
  function caption() {
    const css = getComputedStyle(root);
    const read = (name, fallback) => (css.getPropertyValue(name).trim() || fallback);
    const bg = read("--caption", "#000000");
    const text = read("--caption-text", luminance(bg) > 0.35 ? "#111111" : "#f2f2f2");
    const border = hexToRgb(read("--border", "")) ? read("--border", "") : bg;
    return { dark: luminance(bg) < 0.35, caption: bg, text, border };
  }

  return { apply, caption, style, luminance };
})();
