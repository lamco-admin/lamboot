#!/usr/bin/env python3
"""LamBoot layer-contract checker — the enforcement the contract always needed.

Two rules, both against tools/layer-map.toml (the authoritative map):

  1. DECLARATION: every lamboot-core/src/*.rs module must carry a module-level
     `//! Layer: N` doc comment whose N matches the map.

  2. DIRECTION: a module may `use crate::X` only when layer(X) <= layer(self),
     except (a) X is a cross-cutting module (any layer may use it), or (b) the
     edge is within a declared pure pair (pure half ↔ io shell share a layer).

Exit 0 = clean. Exit 1 = violations (printed). `--graph` prints the DAG + the
topological depth and exits 0 if acyclic.

No third-party deps: a tiny TOML reader covers the simple [table] key=value /
key=[list] shape of layer-map.toml so the gate runs anywhere Python 3 does.
"""
import os
import re
import sys
import glob

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "lamboot-core", "src")
MAP = os.path.join(ROOT, "tools", "layer-map.toml")


def read_map(path):
    """Minimal TOML reader for layer-map.toml's [table] key=int / key="s" / key=[..] shape."""
    layers, cross, pairs = {}, [], {}
    section = None
    for raw in open(path, encoding="utf-8"):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip()
            continue
        if "=" not in line:
            continue
        key, val = (s.strip() for s in line.split("=", 1))
        if section == "layers":
            layers[key] = int(val)
        elif section == "cross_cutting" and key == "modules":
            cross = re.findall(r'"([^"]+)"', val)
        elif section == "pure_pairs":
            pairs[key] = val.strip().strip('"')
    return layers, set(cross), pairs


def strip_comments(src):
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.DOTALL)
    return "\n".join(ln.split("//")[0] for ln in src.splitlines())


def module_deps(modfile, modset):
    src = strip_comments(open(modfile, encoding="utf-8", errors="replace").read())
    d = set()
    for m in re.finditer(r"use\s+crate::([a-z_][a-z0-9_]*)", src):
        if m.group(1) in modset:
            d.add(m.group(1))
    for grp in re.finditer(r"use\s+crate::\{([^}]*)\}", src, flags=re.DOTALL):
        for tok in re.finditer(r"([a-z_][a-z0-9_]*)::", grp.group(1)):
            if tok.group(1) in modset:
                d.add(tok.group(1))
    for m in re.finditer(r"\bcrate::([a-z_][a-z0-9_]*)::", src):
        if m.group(1) in modset:
            d.add(m.group(1))
    return d


def declared_layer(modfile):
    """Return the N from the first `//! Layer: N` line, or None."""
    for ln in open(modfile, encoding="utf-8", errors="replace"):
        m = re.match(r"^//!\s*Layer:\s*(\d+)", ln)
        if m:
            return int(m.group(1))
        # stop scanning once past the module doc-comment header
        if ln.strip() and not ln.lstrip().startswith("//!"):
            break
    return None


def main():
    layers, cross, pairs = read_map(MAP)
    modfiles = {os.path.basename(p)[:-3]: p for p in glob.glob(os.path.join(SRC, "*.rs"))}
    modset = set(modfiles)
    deps = {m: module_deps(p, modset) for m, p in modfiles.items()}

    if "--graph" in sys.argv:
        depth = {}

        def dfs(m, stack):
            if m in depth:
                return depth[m]
            if m in stack:
                print(f"CYCLE: {' -> '.join(list(stack) + [m])}")
                sys.exit(1)
            stack = stack | {m}
            depth[m] = 0 if not deps[m] else 1 + max(dfs(x, stack) for x in deps[m])
            return depth[m]

        for m in modset:
            dfs(m, set())
        for m in sorted(modset, key=lambda m: (depth[m], m)):
            print(f"  depth {depth[m]}  {m:26} -> {' '.join(sorted(deps[m]))}")
        print(f"\nacyclic DAG, max depth {max(depth.values())}")
        sys.exit(0)

    errors = []

    # Rule 0: every mapped module exists and vice versa.
    for m in modset - set(layers):
        errors.append(f"module '{m}' has no entry in tools/layer-map.toml")
    for m in set(layers) - modset:
        errors.append(f"layer-map.toml lists '{m}' but no such module exists")

    # Rule 1: declaration present and matches the map.
    for m, p in sorted(modfiles.items()):
        dl = declared_layer(p)
        ml = layers.get(m)
        if dl is None:
            errors.append(f"{m}.rs: missing mandated `//! Layer: N` declaration")
        elif ml is not None and dl != ml:
            errors.append(f"{m}.rs: declares Layer {dl} but layer-map.toml says {ml}")

    # Rule 2: direction.
    for m in sorted(modset):
        ml = layers.get(m)
        if ml is None:
            continue
        for d in sorted(deps[m]):
            if d in cross:
                continue
            # within-pair pure→io is allowed
            if pairs.get(d) == m or pairs.get(m) == d:
                continue
            dl = layers.get(d)
            if dl is None:
                continue
            if dl > ml:
                errors.append(
                    f"DIRECTION: {m} (L{ml}) imports {d} (L{dl}) — upward dependency"
                )

    if errors:
        print(f"layer check FAILED — {len(errors)} issue(s):")
        for e in errors:
            print(f"  - {e}")
        sys.exit(1)
    print(f"layer check OK — {len(modset)} modules, all declared, no upward dependencies")
    sys.exit(0)


if __name__ == "__main__":
    main()
