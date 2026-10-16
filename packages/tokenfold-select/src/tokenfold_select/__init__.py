"""Optional development-only scorer; importing never loads a model/framework."""
from .artifact import inspect_artifact
from .runtime import DevelopmentScorer

__all__ = ["DevelopmentScorer", "inspect_artifact"]
__version__ = "0.0.0.dev0"
