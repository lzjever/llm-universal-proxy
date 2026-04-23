import importlib.util
import pathlib
import sys
import unittest
from unittest import mock


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_live_surface_fail_fast_contract",
        SCRIPT_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class LiveSurfaceFailFastContractTests(unittest.TestCase):
    def test_fetch_live_model_profile_requires_supports_view_image(self):
        module = load_module()

        with mock.patch.object(
            module,
            "http_get_json",
            return_value={
                "id": "minimax-openai",
                "llmup": {
                    "surface": {
                        "modalities": {"input": ["text"]},
                        "tools": {
                            "supports_search": False,
                            "apply_patch_transport": "freeform",
                            "supports_parallel_calls": False,
                        },
                    },
                },
            },
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "llmup\\.surface\\.tools\\.supports_view_image",
            ):
                module.fetch_live_model_profile(
                    "http://127.0.0.1:18888",
                    "minimax-openai",
                )

    def test_fetch_live_model_profile_requires_supports_parallel_calls(self):
        module = load_module()

        with mock.patch.object(
            module,
            "http_get_json",
            return_value={
                "id": "minimax-openai",
                "llmup": {
                    "surface": {
                        "modalities": {"input": ["text"]},
                        "tools": {
                            "supports_search": False,
                            "supports_view_image": False,
                            "apply_patch_transport": "freeform",
                        },
                    },
                },
            },
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "llmup\\.surface\\.tools\\.supports_parallel_calls",
            ):
                module.fetch_live_model_profile(
                    "http://127.0.0.1:18888",
                    "minimax-openai",
                )


if __name__ == "__main__":
    unittest.main()
