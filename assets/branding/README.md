# ClipForge visual assets

The angular C, play cutout, forge spark and empty-state illustrations were drawn specifically for this local ClipForge fork on 2026-10-13. They contain no third-party logo. The SVG files are editable source; `clipforge-icon.svg` is the Tauri icon-generator input. The React wordmark uses the bundled Inter font.

Interface glyph paths come from Bootstrap Icons 1.13.1 (MIT), already included in the application dependency lock. `generate-icons.mjs` extracts the selected local SVG paths without a network request or icon font. Attribution and the full MIT license are retained beside this file.

Inter 4.1 is bundled in `public/fonts`, with its SIL Open Font License and SHA-256 provenance. It is loaded from local application files only.
