"""Runtime artifact validation, including when Python optimization is enabled."""


def require(condition, message="Artifact validation failed"):
    if not condition:
        raise ValueError(message)
