# Building and publishing the docs

The site uses Sphinx, MyST Markdown, and the Read the Docs theme.
Its version comes from `Cargo.toml`; building the user guide does not compile Rust
or require GPU libraries.

## Build locally

From the repository root, using Python 3.13:

```sh
python3.13 -m venv .venv-docs
.venv-docs/bin/python -m pip install -r docs/requirements.txt
.venv-docs/bin/python -m sphinx -b html -W --keep-going docs docs/_build/html
```

Open `docs/_build/html/index.html`, or serve the output:

```sh
.venv-docs/bin/python -m http.server 8000 --directory docs/_build/html
```

Then visit <http://localhost:8000>. On Windows, the virtual environment's Python
executable is `.venv-docs/Scripts/python.exe`.

The build treats warnings as errors. CircleCI runs the same build in a separate
`docs` job and stores the HTML as an artifact.

## Edit pages

Add Markdown pages under `docs/` and list them in the appropriate toctree in
`index.md`. Several pages include sections of the repository README to avoid
maintaining two copies; edit those sections in `README.md`. Their include
boundaries depend on the README headings.

Existing DriveSuma and surface QC guides are included directly in navigation.
Planning notes and internal UI references are excluded from the published site.
Use relative links for other documentation pages and GitHub URLs for repository
files outside the documentation tree.

## Connect Read the Docs

After these files are pushed to GitHub:

1. Sign in to [Read the Docs](https://app.readthedocs.org/).
2. Import the `pmolfese/sumaru` repository through the GitHub integration.
3. Use the root `.readthedocs.yaml` configuration.
4. Select the branch containing this configuration for the first build. Once
   merged, use `main` as the default branch.
5. Trigger a build and check that the HTML, navigation, and search work.
6. Enable pull-request previews in the project's settings if desired.

The hosted URL is assigned by Read the Docs when the project is created.
Do not advertise a public docs URL until that build succeeds.

See the official [Sphinx deployment guide](https://docs.readthedocs.com/platform/stable/intro/sphinx.html)
and [configuration reference](https://docs.readthedocs.com/platform/stable/config-file/v2.html).
