"""Pytest plugin for ninot-github."""

from collections.abc import Iterator

import pytest

from . import Server


@pytest.fixture
def ninot_github() -> Iterator[Server]:
    """Start a fake GitHub server for testing."""
    server = Server()
    yield server
    server.stop()
