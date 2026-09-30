# kubstu_web_server

Небольшой HTTP-сервер на Rust для Django-приложений через WSGI или ASGI. Сетевой слой построен на Tokio и Hyper, вызовы Python выполняются в отдельном пуле потоков.

## Возможности

- запуск Django-приложения в формате `module:callable`;
- интерфейсы `wsgi` и `asgi`;
- несколько процессов-серверов через `--workers`;
- настройка потоков Tokio и Python, ограниченной очереди Python и максимального размера тела запроса;
- тестовое Django-приложение с HTTP-методами и быстрым endpoint `/rps_plain`;
- скрипты smoke/integration-проверок и воспроизводимые бенчмарки для пяти методов и их интеграции.

## Требования

- Rust toolchain (Cargo и `rustc`);
- Python с Django, доступный для сборки PyO3;
- `wrk` 4.x для бенчмарков;
- Granian для интеграционного сравнения.

Команды ниже выполняются из корня репозитория. Если виртуального окружения ещё нет, создайте его и установите Django:

```bash
python3 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip django
```

Сборка использует Python из `.venv`, чтобы PyO3 и Django запускались в одном окружении:

```bash
PYO3_PYTHON="$PWD/.venv/bin/python" cargo build --release
```

После изменения версии Python пересоберите бинарник с тем же `PYO3_PYTHON`. Для локальной разработки вместо `--release` можно использовать `cargo build`.

## Запуск тестового Django-приложения

Активируйте `.venv` в терминале либо задайте `VIRTUAL_ENV`, как ниже. По умолчанию сервер слушает `127.0.0.1:8000` и использует ASGI:

```bash
VIRTUAL_ENV="$PWD/.venv" cargo run --release -- \
  --interface asgi \
  config.asgi:application \
  --working-dir ./django_test_app \
  --host 127.0.0.1 \
  --port 8000
```

Для WSGI откройте второй терминал и запустите приложение на другом порту:

```bash
VIRTUAL_ENV="$PWD/.venv" cargo run --release -- \
  --interface wsgi \
  config.wsgi:application \
  --working-dir ./django_test_app \
  --host 127.0.0.1 \
  --port 8001
```

Остановите сервер сочетанием `Ctrl+C`. Приложение можно передать и как другой `module:callable`; каталог приложения добавляется в `sys.path` через `--working-dir`.

### Параметры сервера

```text
--interface asgi|wsgi             интерфейс (по умолчанию asgi)
--host 127.0.0.1                  адрес прослушивания
--port 8000                       порт
--workers 2                       число процессов (по умолчанию 1)
--runtime-threads 2               Tokio-потоки на процесс (по умолчанию 1)
--python-threads 4                Python-потоки на процесс (по умолчанию 4)
--python-queue-capacity 256       размер очереди Python на процесс
--max-body-bytes 8388608          максимальное тело запроса (8 MiB)
--working-dir ./django_test_app   каталог приложения Python
```

Пример запуска WSGI с двумя процессами:

```bash
VIRTUAL_ENV="$PWD/.venv" cargo run --release -- \
  --interface wsgi config.wsgi:application \
  --working-dir ./django_test_app \
  --host 127.0.0.1 --port 8001 \
  --workers 2 --runtime-threads 2 --python-threads 2
```

Переполнение очереди Python возвращает HTTP 503, слишком большое тело запроса — HTTP 413. Указанные потоки и ёмкость задаются отдельно для каждого процесса.

## Проверки

Проверка форматирования, unit/integration-тестов Rust и предупреждений Clippy:

```bash
cargo fmt --all -- --check
PYO3_PYTHON="$PWD/.venv/bin/python" cargo test --all-targets
PYO3_PYTHON="$PWD/.venv/bin/python" cargo clippy --all-targets -- -D warnings
```

Скрипт протокольных регрессий сам запускает тестовые WSGI- и ASGI-приложения. Перед ним должна быть собрана release-версия:

```bash
PYO3_PYTHON="$PWD/.venv/bin/python" cargo build --release
VIRTUAL_ENV="$PWD/.venv" .venv/bin/python scripts/integration_check.py
```

`integration_check.py` проверяет ответы WSGI/ASGI, ограничение тела (413) и переполнение очереди (503). Smoke-проверка семи HTTP-методов обращается к уже запущенному Django-приложению:

```bash
.venv/bin/python scripts/http_methods_check.py --port 8000
```

Укажите порт WSGI-сервера, если проверяете его (`--port 8001`). Ожидаемые маршруты Django находятся в `django_test_app/config/urls.py`.

## Бенчмарки методов

Каталоги `benchmarks/01_shared_listener_inheritance` — `benchmarks/05_runtime_isolation` сравнивают базовый Rust WSGI-сервер с одним методом из статьи за раз; каждому методу назначен профиль нагрузки по его цели. `benchmarks/06_integrated` сравнивает сервер со всеми пятью методами против Granian на смешанном трафике. В каждой папке лежат профиль `comparison.json`, исполняемый `run.sh`, описание и сохранённый `results.json`. Worker Inheritance и интеграционный прогон также измеряют 40 холодных стартов на вариант.

После установки зависимостей запустите нужное сравнение из корня репозитория:

```bash
benchmarks/01_shared_listener_inheritance/run.sh
benchmarks/02_worker_inheritance/run.sh
benchmarks/03_execution_pool/run.sh
benchmarks/04_zero_copy/run.sh
benchmarks/05_runtime_isolation/run.sh
benchmarks/06_integrated/run.sh
```

Понадобятся `.venv` с Django и Granian, Rust/Cargo и `wrk` в `PATH`. Каждый запуск пересобирает release-сервер и записывает свежие метрики в `results.json` соответствующего каталога; сохранённые JSON являются результатами последней серии. Полная методика, профили нагрузки, аппаратные характеристики стенда и ограничения приведены в [docs/benchmark-methodology-ru.md](docs/benchmark-methodology-ru.md).

## Структура проекта

- `src/` — Rust HTTP-сервер и мост к Python/WSGI/ASGI.
- `django_test_app/` — тестовый Django-проект и лёгкий маршрут `/rps_plain`.
- `tests/fixture_apps.py` — минимальные приложения для протокольных интеграционных проверок.
- `scripts/` — smoke-проверки, регрессии, нагрузочный и startup-бенчмарки с общими профилями.
- `benchmarks/` — пять изолированных сравнений и итоговое сравнение с Granian.
- `docs/` — методика нагрузочного тестирования и описание ограничений эксперимента.
