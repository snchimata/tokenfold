"""Run with the freshly installed lightweight runtime; no source-path fallback."""
import builtins
import importlib.metadata
import sys

original = builtins.__import__


def guarded(name, *args, **kwargs):
    assert name.split(".")[0] not in {"torch", "transformers", "tokenizers", "huggingface_hub"}
    return original(name, *args, **kwargs)


builtins.__import__ = guarded
def offline(event, args):
    assert event not in {"socket.connect", "socket.getaddrinfo", "subprocess.Popen"}, event


sys.addaudithook(offline)
import tokenfold_select_runtime as runtime

distribution = importlib.metadata.distribution("tokenfold-select-runtime")
assert runtime.__version__ == distribution.version == "0.6.0"
assert "site-packages" in runtime.__file__
assert not distribution.requires
assert distribution.metadata["Requires-Python"] == ">=3.12"
names = {str(path).replace("\\", "/") for path in distribution.files}
for suffix in ("LICENSE", "NOTICE", "PROVENANCE.json"):
    assert any(name.endswith("/" + suffix) for name in names), suffix
assert not any(name.endswith((".safetensors", ".onnx")) for name in names)
print("Installed lightweight runtime metadata/import/license/provenance passed; no ML loaded")
