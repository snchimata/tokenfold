"""Optional development-only scorer; importing never loads a model/framework."""
from .artifact import inspect_artifact
from .runtime import DevelopmentScorer
from .onnx import OnnxScorer, inspect_onnx

__all__ = ["OnnxScorer", "inspect_onnx", "DevelopmentScorer", "inspect_artifact"]
__version__ = "0.5.2"
