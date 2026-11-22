import json
import os
import struct
import subprocess
import sys
import urllib.request


def write_frame(stream, frame):
    body = json.dumps(frame, separators=(",", ":")).encode()
    stream.write(struct.pack(">I", len(body)) + body)
    stream.flush()


def read_frame(stream):
    prefix = stream.read(4)
    if len(prefix) < 4:
        return None
    length = struct.unpack(">I", prefix)[0]
    if length == 0 or length > (1 << 20):
        raise RuntimeError("bad frame length %d" % length)
    body = stream.read(length)
    if len(body) != length:
        return None
    return json.loads(body)


def post(url, payload):
    data = json.dumps(payload).encode()
    request = urllib.request.Request(
        url, data=data, headers={"Content-Type": "application/json"}, method="POST"
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.loads(response.read())


def main():
    adapter, scenarios_dir, api_base, profile, scenario = sys.argv[1:6]
    env = dict(os.environ)
    env["SMOLRUNNERS_SCENARIOS_DIR"] = scenarios_dir
    process = subprocess.Popen(
        [adapter, "--fake"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    write_frame(
        process.stdin,
        {
            "kind": "run",
            "schema_version": 1,
            "id": "run-cli-e2e",
            "scenario": scenario,
        },
    )
    demands = []
    frames = 0
    while frames < 200:
        frame = read_frame(process.stdout)
        if frame is None:
            break
        frames += 1
        kind = frame.get("kind")
        if kind == "session" and frame.get("session_event") == "adapter_stopped":
            break
        if kind in ("job_available", "job_assigned"):
            demands.append(
                {
                    "protocol_job_id": frame.get("job_id", "job-unknown"),
                    "request_id": frame.get("request_id", 1),
                    "job_key": frame.get("job_id", "build"),
                    "profile": profile,
                    "labels": frame.get("labels", []),
                }
            )
        try:
            write_frame(
                process.stdin,
                {
                    "kind": "ack",
                    "schema_version": 1,
                    "id": "ack-" + frame.get("id", "unknown"),
                    "event_id": frame.get("id", "unknown"),
                    "decision": "accept",
                    "recorded": True,
                },
            )
        except BrokenPipeError:
            break
    try:
        process.wait(timeout=20)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    if not demands:
        raise RuntimeError("fakebroker emitted no demand frames")
    posted = [post(api_base + "/api/v1/demand", demand)["data"] for demand in demands]
    print(json.dumps({"frames": frames, "demands": demands, "jobs": posted}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("fakebroker_demand: %s" % error, file=sys.stderr)
        sys.exit(1)
