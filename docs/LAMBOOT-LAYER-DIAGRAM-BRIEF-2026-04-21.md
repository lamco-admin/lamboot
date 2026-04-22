# LamBoot Architecture — Visual Layer Diagram Brief

**Date:** 2026-04-21
**Audience:** LAMCO-website team (Django/Cotton templates, design assets)
**Purpose:** spec a visual representation of LamBoot's 8-layer code architecture, usable on lamco.ai as a prominent content element
**Companion:** `docs/ARCHITECTURE-LAYERS.md` (the authoritative code-side layer model)

---

## 1. Why a visual matters

The existing `lamco.ai/products/lamboot/architecture/` page has one ASCII diagram — but it depicts **physical deployment topology** (Host OS / ESP / UEFI firmware) rather than the **code architecture** (the 8 module layers). These are different axes.

A clean layer diagram on the site does three things at once:
- Proves the software was designed, not evolved by accretion.
- Gives reviewers, contributors, and security auditors a single-glance mental model.
- Creates a distinctive visual identity for LamBoot — no other Linux bootloader project ships a layer diagram like this, because most can't.

The diagram must convey: **clean layering, unidirectional dependency, and small surface per layer.** Those are the architectural claims the code backs up.

## 2. What to show

Eight horizontally-banded layers stacked bottom-up. Lower layers sit below; higher layers above. **Arrows point only downward** (higher layer *uses* lower layer). Cross-cutting Layer 5 (Trust & Audit) is drawn as a **right-hand vertical rail** that any layer can *write to* (single-direction arrow into the rail, never out).

### 2.1 Layer inventory (bottom → top)

| # | Layer | One-line role | Example modules |
|---|---|---|---|
| 0 | **Platform Introspection** | Read-only discovery of hardware / firmware context | `acpi`, `hypervisor`, `smbios`, `fw_cfg` |
| 1 | **UEFI Firmware Boundary** | All UEFI protocol calls live here | `fs`, `partitions`, `tpm`, `secure`, `drivers`, `security_override`, `initrd` |
| 2 | **Filesystem Abstraction** | Uniform read API across FAT, ext4 (v0.9), Btrfs (v1.1+) | `fs_backend`, `fs_backend_fat`, `fs_backend_ext4` |
| 3 | **Content Parsers** | Pure bytes-to-structures, no I/O | `bls`, `uki`, `policy` (parse), `pe_loader` (v0.9+) |
| 4 | **Policy & State** | Config-driven rules + NVRAM state | `policy`, `health`, `autodiscovery`, `preflight` |
| 5 | **Trust & Audit** (cross-cutting rail) | Append-only decision record | `trust_log`, `bootlog`, `report`, `telemetry` |
| 6 | **Presentation** | GUI, serial console, input dispatch | `gui`, `console`, `input` |
| 7 | **Orchestration** | The conductor — composes the rest | `main`, `discovery`, `boot` |

### 2.2 ASCII mockup (what the graphic represents)

This is the conceptual reference, not the final visual. The designer's job is to make this elegant.

```
┌──────────────────────────────────────────────────────────┐  ┌───────────────┐
│   7  Orchestration      main.rs · discovery · boot       │  │               │
├──────────────────────────────────────────────────────────┤  │               │
│   6  Presentation       gui · console · input            │  │               │
├──────────────────────────────────────────────────────────┤  │               │
│   4  Policy & State     policy · health · preflight      │←→│  5  Trust &   │
├──────────────────────────────────────────────────────────┤  │     Audit     │
│   3  Content Parsers    bls · uki · policy·parse · pe    │←→│   (append-    │
├──────────────────────────────────────────────────────────┤  │    only rail) │
│   2  FS Abstraction     fs_backend · fat · ext4 · btrfs  │←→│               │
├──────────────────────────────────────────────────────────┤  │  trust_log    │
│   1  UEFI Firmware      fs · partitions · tpm · …        │←→│  bootlog      │
├──────────────────────────────────────────────────────────┤  │  report       │
│   0  Platform           acpi · hypervisor · smbios · …   │←→│  telemetry    │
└──────────────────────────────────────────────────────────┘  └───────────────┘
                                   ↓                                 ↑
                         dependencies flow down         writes flow right
```

Layers 0 through 7 are a vertical stack. Layer 5 is a vertical rail on the right — any layer can append to it. **No arrows ever go upward**, and **no arrows come *out* of the Trust & Audit rail** (it's a write-only sink).

## 3. Design guidance for the web implementation

### 3.1 Must-haves

- **8 visible bands** for layers 0–7 in the main stack.
- **1 vertical rail** on the right for Layer 5 (Trust & Audit), visually distinct from the main stack (different background or border treatment).
- **Each layer band** shows: layer number, layer name, one-line role, and 3–4 representative module names (not all — keep readable).
- **Downward dependency arrows** between adjacent layers (can be small chevrons on the band boundary).
- **Write arrows** from each main layer into the Trust & Audit rail (can be thin horizontal lines with arrowheads).
- **Legend** beneath: "Higher layers use lower layers. Trust & Audit is append-only — written to, never read by logic."
- **Version callout** somewhere: "Layer 2 native backends (ext4, FAT) land in v0.9.x. Layer 3 `pe_loader` lands in v0.9.x. See roadmap."

### 3.2 Visual treatment options (designer picks)

**Option A — Flat solid bands.** Eight horizontal rectangles, monospace-style module names, subtle drop shadow, arrow chevrons on boundaries. Clean, bookish, matches a technical-documentation aesthetic. Scales down on mobile by collapsing module lists into dots.

**Option B — Elevation blocks.** Each layer is a shallow 3D block (slight isometric tilt), giving the "stack" metaphor physical presence. Trust & Audit rail is a complementary column. More visually striking; heavier on mobile.

**Option C — Interactive hover.** Any of the above, plus hover-reveal: mousing over a layer expands it to show full module list and a short "what this layer does" paragraph. Click a module name → jump to the architecture doc section.

**Recommended: Option A + Option C's hover behavior.** Clean default, richer on engagement, accessible (keyboard-focusable), mobile-graceful.

### 3.3 Color semantics (keep small palette)

- **Layers 0–1 (physical-ish):** cool neutral — slate, stone, or muted blue.
- **Layers 2–4 (data / logic):** brand primary tint stepped by layer (lighter at top).
- **Layer 5 rail:** brand accent (distinct from main stack; suggests "different kind of thing").
- **Layers 6–7 (user-visible / conductor):** warmer tone.

Avoid red on any "working" element (reserve for errors). Avoid green on any single layer (could imply "this layer is the good one"). Neutrality across layers matches the "every layer does its own job" message.

### 3.4 Typography

- Module names in a monospace face (they are file names and identifiers).
- Layer names in the site's existing display face (Space Grotesk per current theme).
- Layer number in a large display weight for scan-ability.

### 3.5 Mobile behavior

- Stack bands vertically at full width; collapse module lists to "tap to expand".
- Trust & Audit rail moves below the main stack on narrow viewports with a caption "Every layer writes to the Trust & Audit record."
- Arrows become simpler (short vertical lines with downward chevrons).

## 4. Where the diagram lives on the site

### 4.1 Primary placement — `lamco.ai/products/lamboot/architecture/`

**Above the existing ASCII "Layer Overview" block (which is physical deployment, not code architecture).** Rename the existing block to "Deployment Topology" or "Runtime Topology" so the two diagrams don't conflict. The code-architecture diagram is the headline visual; the deployment topology follows as supporting detail.

### 4.2 Secondary placements (condensed variants)

- **Home page (`/products/lamboot/`)** — a small non-interactive thumbnail in the "What makes it different" section, with link to the architecture page for the full version. Shows just layer numbers + names, no modules. Purpose: visual preview, not an explanation.
- **Innovations page (`/products/lamboot/innovations/`)** — same condensed variant in the "Architecture efficiency" innovation section, reinforcing that item.
- **Developers / contribute section** — the full interactive version, because new contributors need the layer model to understand where to add code.

### 4.3 Do NOT place on

- Install page (wrong audience).
- FAQ (wrong format).
- Security page (would reinforce the security-first framing the site is moving away from — see corrections doc).

## 5. Accompanying copy

The diagram needs a short intro paragraph. Suggested:

> **Every module in LamBoot declares its layer.** The bootloader is divided into 8 layers, each with a single responsibility. Higher layers build on lower; dependencies never flow the other way. A separate Trust & Audit rail records every decision to a JSON log on disk — append-only, never consulted as control flow. This discipline is why LamBoot is ~8,300 lines of Rust instead of 40,000+ lines of C, and why every new feature slots cleanly into one existing layer rather than cutting across several.

Then the diagram. Then (collapsed-by-default) per-layer notes pulled from `docs/ARCHITECTURE-LAYERS.md`.

## 6. Versioning the diagram

The code architecture evolves:

- **v0.8.3 (today):** Layer 2 is a single `fs.rs` module. Layer 3 has three parsers.
- **v0.9.x:** Layer 2 gains `fs_backend_fat`, `fs_backend_ext4`; Layer 3 gains `pe_loader`.
- **v1.0:** same shape as v0.9.x, refined.
- **v1.1+:** Layer 2 may add `fs_backend_btrfs`, `fs_backend_xfs` if community contributes.

The diagram should be **re-exportable per release**. Recommended: keep the source (Figma / SVG with editable layer names) in the website repo so each release can bump it with minimal effort.

## 7. What NOT to include

- **No firmware internals.** UEFI Security2Arch / ShimLock / BS->LoadImage are not part of LamBoot's architecture — they are dependencies Layer 1 calls. Showing them would clutter the "this is LamBoot's code" message.
- **No Linux kernel or initrd.** These are targets, not layers.
- **No hypervisor / VM state.** Covered by the deployment-topology diagram.
- **No "C code badge" or "Rust mascot" stickers.** The diagram is a technical artifact; let the rest of the page do brand work.

## 8. Deliverables expected from the website team

| Deliverable | Format | Used where |
|---|---|---|
| Master SVG (editable) | SVG, layered in Figma | Source for all variants |
| Full interactive diagram (hover, keyboard-focus) | Web component (Cotton template + inline SVG + small JS) | Architecture page |
| Condensed thumbnail variant | Static SVG | Home + Innovations |
| Static PNG fallback (JS-disabled, print) | 2x Retina PNG | Architecture page, press kit |
| Short accompanying copy (50–100 words) | Markdown snippet | All placements |
| Per-release bump procedure | One-paragraph internal note | Website team's maintenance docs |

## 9. Acceptance criteria (for the web team to self-check)

A reviewer with no prior context should be able to, in under 30 seconds:
- Name the 8 layers without reading prose.
- Identify which layer a new module like `fs_backend_btrfs` would belong to.
- State that Trust & Audit is write-only.
- See that dependencies flow one direction.

If any of those require scrolling to a prose paragraph, the diagram isn't carrying its weight.

---

**Pair with:** `docs/LAMBOOT-WEBSITE-CONTENT-REAUDIT-2026-04-21.md` (the corrections that accompany this diagram's introduction).
