# Third-Party Notices

Mergeon is licensed under the GNU General Public License v3.0 or later (`GPL-3.0-or-later`).
This file documents third-party software, fonts, icons, and libraries included in or distributed
with Mergeon.

---

## GPUI (`native/vendor/gpui-pre`)

Mergeon includes a modified copy of `gpui-pre 0.3.7` (snapshot of Zed's `gpui` crate at commit
`1a28cff4b409169bac058bca40dfbfeb7621d19b`), licensed under the **Apache License 2.0**.

- Copyright 2022–2025 Zed Industries, Inc.
- Upstream repository: <https://github.com/zed-industries/zed>
- Full license text: [`vendor/gpui-pre/LICENSE-APACHE`](vendor/gpui-pre/LICENSE-APACHE)
- Summary of Mergeon modifications: [`vendor/gpui-pre/MERGEON.md`](vendor/gpui-pre/MERGEON.md)

All modifications made by Mergeon contributors inside `native/vendor/gpui-pre` are also
licensed under the Apache License 2.0 to remain compatible with upstream Zed.

---

## GPUI Kit (`gpui-kit`, `gpui-component`, `gpui-kit-assets`)

- License: **Apache-2.0**
- Includes Lucide icons (`ISC` license) via `gpui-kit-assets`.

---

## Geist & Geist Mono Typefaces

Bundled in `native/crates/mergeon-ui/assets/fonts/`.

- Copyright 2023–2025 Vercel, Inc. / basement.studio
- License: **SIL Open Font License 1.1 (OFL-1.1)**
- Full license text: [`crates/mergeon-ui/assets/fonts/OFL.txt`](crates/mergeon-ui/assets/fonts/OFL.txt)

---

## Lucide Icons

Used via `gpui-kit-assets` and `native/crates/mergeon-view-sidebar/assets/star-filled.svg`.

- Copyright (c) for portions of Lucide are held by Cole Bemis 2013–2022 as part of Feather (MIT).
  All other copyright (c) for Lucide are held by Lucide Contributors 2022–present.
- License: **ISC License**
- Upstream: <https://lucide.dev/>

---

## Heroicons

Toast notification icons in `native/crates/mergeon-ui/assets/icons/toast-{error,info,success,warning}.svg`.

- Copyright (c) Tailwind Labs, Inc.
- License: **MIT License**
- Upstream: <https://heroicons.com/>

---

## `ring` Cryptography Library

Used for Ed25519 manifest signature verification and TLS (`rustls`).

- License: **Apache-2.0 AND ISC** (plus BoringSSL/OpenSSL notices)
- Upstream: <https://github.com/briansmith/ring>

---

## MPL-2.0 Rust Dependencies

The following unmodified crates from [crates.io](https://crates.io/) are licensed under the
**Mozilla Public License 2.0 (MPL-2.0)**; their exact versions and source locations are pinned
in [`native/Cargo.lock`](Cargo.lock):

- `cbindgen` (build-time tool) — <https://crates.io/crates/cbindgen>
- `dwrote` (Windows DirectWrite bindings) — <https://crates.io/crates/dwrote>
- `option-ext` — <https://crates.io/crates/option-ext>

---

## gitcraft

Several design ideas and implementation details introduced in Mergeon 0.13 were inspired by
[gitcraft](https://github.com/Vancete/gitcraft), a Git client in Rust published under
**MIT OR Apache-2.0**. Everything was reimplemented for Mergeon's architecture; no UI code was
copied.

- `mergeon-gitcmd/src/credentials.rs`: inline `credential.helper` reading credentials from an
  ephemeral file, and the list of Git HTTP authentication error messages.
- `mergeon-git-query/src/images.rs`: which image revisions are compared in each view (commit,
  index, working tree, range).
- `mergeon-git-query/src/diff.rs`: requesting whole-file context via `--unified=999999999`.
- General undo approach storing `HEAD` before/after mutations and running inverse commands for
  branches, tags, and stashes.

```text
MIT License

Copyright (c) 2026 Iván

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
