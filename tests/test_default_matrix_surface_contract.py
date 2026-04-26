import importlib.util
import pathlib
import sys
import tempfile
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
CONFIG_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "default_proxy_test_matrix.yaml"
)
PRESET_ENV = {
    "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
    "PRESET_OPENAI_ENDPOINT_BASE_URL": "https://openai-compatible.example/v1",
    "PRESET_ANTHROPIC_ENDPOINT_BASE_URL": "https://anthropic-compatible.example/v1",
    "PRESET_ENDPOINT_MODEL": "provider-configured-model",
}


def load_module():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_default_surface_contract",
        SCRIPT_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def effective_surface_for_model(module, config, model_name: str):
    alias_config = config.model_alias_configs.get(model_name)
    if alias_config is None:
        upstream_name = module._target_upstream_name(model_name)
        alias_surface = None
    else:
        upstream_name = module._target_upstream_name(alias_config.target)
        alias_surface = alias_config.surface
    upstream_surface = (
        config.upstream_surface_defaults.get(upstream_name)
        if upstream_name is not None
        else None
    )
    return module._effective_surface_metadata(upstream_surface, alias_surface)


class DefaultMatrixSurfaceContractTests(unittest.TestCase):
    def test_primary_default_matrix_lanes_provide_live_surface_requirements(self):
        module = load_module()
        parsed = module.parse_proxy_source(CONFIG_PATH.read_text(encoding="utf-8"))

        self.assertIn("PRESET-ANTHROPIC-COMPATIBLE", parsed.upstream_surface_defaults)
        self.assertIn("PRESET-OPENAI-COMPATIBLE", parsed.upstream_surface_defaults)
        self.assertNotIn("PRESET-ANTHROPIC-COMPATIBLE", parsed.upstream_codex_metadata)
        self.assertNotIn("PRESET-OPENAI-COMPATIBLE", parsed.upstream_codex_metadata)

        for model_name in (
            "preset-anthropic-compatible",
            "preset-openai-compatible",
        ):
            with self.subTest(model_name=model_name):
                surface = effective_surface_for_model(module, parsed, model_name)
                module._validate_live_surface_codex_requirements(
                    surface,
                    require_tool_flags=True,
                )
                self.assertEqual(surface.input_modalities, ("text",))
                self.assertFalse(surface.supports_search)
                self.assertFalse(surface.supports_view_image)
                self.assertFalse(surface.supports_parallel_calls)

    def test_runtime_config_preserves_primary_lane_surface_contracts(self):
        module = load_module()
        parsed = module.parse_proxy_source(CONFIG_PATH.read_text(encoding="utf-8"))

        with tempfile.TemporaryDirectory() as temp_dir:
            runtime_text = module.build_runtime_config_text(
                parsed,
                PRESET_ENV,
                listen_host="127.0.0.1",
                listen_port=19999,
                trace_path=pathlib.Path(temp_dir) / "trace.jsonl",
            )
        self.assertIn("supports_parallel_calls: false", runtime_text)

        runtime_parsed = module.parse_proxy_source(runtime_text)
        for model_name in (
            "preset-anthropic-compatible",
            "preset-openai-compatible",
        ):
            with self.subTest(model_name=model_name):
                surface = effective_surface_for_model(module, runtime_parsed, model_name)
                module._validate_live_surface_codex_requirements(
                    surface,
                    require_tool_flags=True,
                )
                self.assertEqual(surface.input_modalities, ("text",))
                self.assertFalse(surface.supports_search)
                self.assertFalse(surface.supports_view_image)
                self.assertFalse(surface.supports_parallel_calls)


if __name__ == "__main__":
    unittest.main()
