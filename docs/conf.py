"""Sphinx configuration shared by local, CircleCI, and Read the Docs builds."""
from pathlib import Path
import os
import tomllib

ROOT = Path(__file__).resolve().parents[1]
package = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]
project = "sumaru"
author = "P. Molfese"
copyright = "United States Government Work"
release = package["version"]
version = release

extensions = ["myst_parser", "sphinx_rtd_theme"]
source_suffix = {".rst": "restructuredtext", ".md": "markdown"}
root_doc = "index"
exclude_patterns = [
    "_build", ".venv", "requirements.txt", "images/README.md",
    "COMPLETED.md", "ROADMAP.md", "EGUI_CONTROLLER_REFERENCE.md",
]
myst_heading_anchors = 4
html_theme = "sphinx_rtd_theme"
html_title = f"sumaru {release}"
html_theme_options = {"navigation_depth": 3, "collapse_navigation": False}
html_show_sourcelink = True
html_baseurl = os.environ.get("READTHEDOCS_CANONICAL_URL", "")
