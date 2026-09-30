from .settings import *  # noqa: F403,F401

# Production-like benchmark profile for server throughput measurements.
DEBUG = False
ALLOWED_HOSTS = ["*"]

# Keep only what is needed for URL dispatch and simple HttpResponse views.
INSTALLED_APPS = []  # noqa: F405
MIDDLEWARE = []
TEMPLATES = []

# No DB is required for the benchmark endpoints.
DATABASES = {}

# The benchmark's large-body profile sends 10 MiB requests. Keep Django's
# request-body guard enabled, but raise it above the benchmark payload size.
DATA_UPLOAD_MAX_MEMORY_SIZE = 12 * 1024 * 1024
