"""Tests for the website-claim verifier.

The verifier runs against a checked-out copy of the lamboot-dev repo
and confirms each SDS-4 §8.1 claim has code-path evidence. These tests
run the verifier against the repo that contains them.
"""
from __future__ import annotations

import pathlib
import sys

HERE = pathlib.Path(__file__).resolve()
sys.path.insert(0, str(HERE.parent.parent))

from lamboot_inspect import verify as verify_mod  # noqa: E402

REPO_ROOT = HERE.parent.parent.parent


def test_claims_table_not_empty():
    assert verify_mod.CLAIMS, "claims table must not be empty"


def test_each_claim_has_evidence():
    for claim in verify_mod.CLAIMS:
        assert claim.evidence, f"claim {claim.name} has no evidence"
        assert claim.spec_section, f"claim {claim.name} has no spec cross-reference"


def test_every_claim_has_unique_name():
    names = [c.name for c in verify_mod.CLAIMS]
    assert len(names) == len(set(names))


def test_verify_runs_without_exception():
    # The on-disk repo in this checkout has SDS-1 merged on the SDS-1
    # branch; on main some evidence files do not exist yet. This test
    # therefore does not require every claim to be substantiated — it
    # only requires check_all to complete without crashing.
    results = verify_mod.check_all(REPO_ROOT)
    assert len(results) == len(verify_mod.CLAIMS)
    for r in results:
        assert r.claim.name in {c.name for c in verify_mod.CLAIMS}


def test_no_std_main_claim_substantiated():
    # This claim points at lamboot-core/src/main.rs which exists on any
    # branch — use it as a sanity check that the verifier mechanics work
    # even when the branch under test lacks SDS-1 files.
    results = {r.claim.name: r for r in verify_mod.check_all(REPO_ROOT)}
    r = results["rust_no_std"]
    assert r.substantiated, f"no_std claim not substantiated: missing {[e.path for e in r.missing]}"
