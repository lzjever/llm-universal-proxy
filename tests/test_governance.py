import pathlib
import re
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
GOVERNANCE_SCRIPT = REPO_ROOT / "scripts" / "check-governance.sh"
ACTIVE_DOC_PATHS = (
    REPO_ROOT / "README.md",
    REPO_ROOT / "README_CN.md",
    *sorted((REPO_ROOT / "docs").glob("*.md")),
)
BOUNDARY_LANGUAGE_RE = re.compile(
    r"\b("
    r"portab\w+|"
    r"native[- ]extension\w*|"
    r"fail[- ]warn|"
    r"warn(?:ing|ings)?|"
    r"reject(?:s|ed|ing)?|"
    r"degrad\w+|"
    r"non[- ]portable|"
    r"boundar(?:y|ies)"
    r")\b",
    re.IGNORECASE,
)
NEGATED_BOUNDARY_LANGUAGE_RE = re.compile(
    r"\b(?:without|no)\s+(?:compatibility\s+)?"
    r"(?:warnings?|warn(?:ing|ings)?|reject(?:ion|ions|s|ed|ing)?)"
    r"(?:\s+or\s+(?:warnings?|warn(?:ing|ings)?|reject(?:ion|ions|s|ed|ing)?))*\b",
    re.IGNORECASE,
)
UNBOUNDED_COMPAT_PROMISE_PATTERNS = (
    (
        "drop-in replacement",
        re.compile(r"\bdrop[- ]in replacement\b", re.IGNORECASE),
    ),
    (
        "exact preservation / zero loss",
        re.compile(r"\bexact preservation\b|\bzero loss\b", re.IGNORECASE),
    ),
    (
        "Any-to-Any",
        re.compile(r"\bAny[- ]to[- ]Any\b", re.IGNORECASE),
    ),
    (
        "full fidelity",
        re.compile(r"\bfull fidelity\b", re.IGNORECASE),
    ),
    (
        "any client / any backend",
        re.compile(
            r"\bany client\b(?:(?!\n\s*\n).){0,220}\bany (?:LLM )?backend\b",
            re.IGNORECASE | re.DOTALL,
        ),
    ),
    (
        "transparent any upstream",
        re.compile(
            r"\btransparen\w*\b(?:(?!\n\s*\n).){0,220}\bany upstream\b",
            re.IGNORECASE | re.DOTALL,
        ),
    ),
    (
        "all 16 as unconditional success",
        re.compile(
            r"\ball 16\b(?:(?!\n\s*\n).){0,180}\b("
            r"full fidelity|work(?:ing|s| correctly)?|pass(?:es|ing)?"
            r")\b",
            re.IGNORECASE | re.DOTALL,
        ),
    ),
)


def has_valid_boundary_language(unit: str) -> bool:
    boundary_text = NEGATED_BOUNDARY_LANGUAGE_RE.sub("", unit)
    return BOUNDARY_LANGUAGE_RE.search(boundary_text) is not None


def claim_units(text: str):
    for paragraph_match in re.finditer(r"(?:[^\n]|\n(?!\s*\n))+", text):
        paragraph = paragraph_match.group(0).strip("\n")
        if not paragraph.strip():
            continue

        table_lines = [
            line for line in paragraph.splitlines() if line.lstrip().startswith("|")
        ]
        if len(table_lines) > 1:
            for line in table_lines:
                yield line, paragraph_match.start() + paragraph_match.group(0).find(line)
        else:
            yield paragraph, paragraph_match.start()


class GovernanceTests(unittest.TestCase):
    def test_governance_tracks_dynamic_proxy_binary_rule(self):
        script = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        self.assertIn(
            'check_contains "scripts/real_cli_matrix.py" "def default_proxy_binary_path("',
            script,
        )
        self.assertIn(
            'check_contains "scripts/real_cli_matrix.py" \'DEFAULT_PROXY_BINARY = default_proxy_binary_path()\'',
            script,
        )
        self.assertIn(
            'check_contains "scripts/interactive_cli.py" \'default=str(default_proxy_binary_path())\'',
            script,
        )
        self.assertNotIn(
            'check_contains "scripts/real_cli_matrix.py" \'DEFAULT_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"\'',
            script,
        )

    def test_active_docs_bound_overbroad_compatibility_promises(self):
        violations = []

        for path in ACTIVE_DOC_PATHS:
            text = path.read_text(encoding="utf-8")
            for unit, start_index in claim_units(text):
                for label, pattern in UNBOUNDED_COMPAT_PROMISE_PATTERNS:
                    if pattern.search(unit) and not has_valid_boundary_language(unit):
                        line_no = text.count("\n", 0, start_index) + 1
                        excerpt = " ".join(unit.strip().split())
                        violations.append(
                            f"{path.relative_to(REPO_ROOT)}:{line_no}: "
                            f"{label}: {excerpt[:180]}"
                        )

        self.assertFalse(
            violations,
            "Unbounded compatibility promises must include same-paragraph "
            "portability/native-extension/fail-warn boundaries:\n"
            + "\n".join(violations),
        )

    def test_overbroad_compatibility_patterns_cover_high_risk_language(self):
        risky_text = "\n\n".join(
            (
                "Text content - exact preservation, zero loss.",
                "Any-to-Any: Every supported client protocol can reach every supported upstream protocol.",
                "The proxy is a drop-in replacement without warning.",
            )
        )
        detected_labels = set()

        for unit, _start_index in claim_units(risky_text):
            for label, pattern in UNBOUNDED_COMPAT_PROMISE_PATTERNS:
                if pattern.search(unit) and not has_valid_boundary_language(unit):
                    detected_labels.add(label)

        self.assertIn("exact preservation / zero loss", detected_labels)
        self.assertIn("Any-to-Any", detected_labels)
        self.assertIn("drop-in replacement", detected_labels)


if __name__ == "__main__":
    unittest.main()
