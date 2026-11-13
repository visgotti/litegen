"""The GIL is released while the converter works, and one model serves many
threads at once, from its first write."""

import threading
import time

import litegen_3d_converter as l3d
from conftest import read_testdata


def grid_obj(n: int = 900) -> bytes:
    """An OBJ grid of 2 x n x n triangles: big enough that one conversion
    takes a measurable time."""
    lines = [f"v {x / n} {((x * y) % 7) / 100} {y / n}" for y in range(n + 1) for x in range(n + 1)]
    for y in range(n):
        for x in range(n):
            a = y * (n + 1) + x + 1
            lines += [f"f {a} {a + n + 1} {a + 1}", f"f {a + 1} {a + n + 1} {a + n + 2}"]
    return ("\n".join(lines) + "\n").encode()


def spinner_progress(work: object) -> tuple[float, float]:
    """How fast a second thread runs Python while `work()` runs in this one,
    against how fast it runs while this thread sleeps (the GIL free), in
    increments per second."""
    counter = 0
    stop = threading.Event()

    def spin() -> None:
        nonlocal counter
        while not stop.is_set():
            counter += 1

    spinner = threading.Thread(target=spin)
    spinner.start()
    try:
        time.sleep(0.05)
        before, started = counter, time.perf_counter()
        time.sleep(0.2)
        free = (counter - before) / (time.perf_counter() - started)
        before, started = counter, time.perf_counter()
        work()  # type: ignore[operator]
        elapsed = time.perf_counter() - started
        during = (counter - before) / elapsed
    finally:
        stop.set()
        spinner.join()
    assert elapsed > 0.1, f"the conversion took {elapsed:.3f}s; it must run long enough to measure"
    return during, free


def test_gil_is_released_during_a_conversion() -> None:
    model = l3d.read_model(grid_obj())
    during, free = spinner_progress(lambda: model.write(to_format="fbx"))
    # One call, so no bytecode boundary hands the GIL over: holding it, the
    # spinner would get one 5 ms switch interval at most, a few percent of
    # the call; released, it runs at close to its free rate.
    assert during > 0.3 * free, f"the spinner ran at {during:,.0f}/s during the call, {free:,.0f}/s free"


def test_concurrent_first_writes_on_one_model_match() -> None:
    data = read_testdata("fbx/textured_assimp_7500.fbx")
    want = l3d.read_model(data).write(to_format="fbx").files[0].data
    # A model no thread has written yet: its first write (transcoding, cached
    # once) and its first `info` race across every thread.
    model = l3d.read_model(data)
    results: list[bytes] = []
    triangles: list[int] = []
    lock = threading.Lock()
    start = threading.Barrier(8)

    def work() -> None:
        start.wait()
        for _ in range(10):
            body = model.write(to_format="fbx").files[0].data
            count = model.info.triangles
            with lock:
                results.append(body)
                triangles.append(count)

    threads = [threading.Thread(target=work) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert len(results) == 80
    assert all(r == want for r in results)
    assert len(set(triangles)) == 1
