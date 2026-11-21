import json
import sys
import urllib.error
import urllib.request


def post(url, payload):
    data = json.dumps(payload).encode()
    request = urllib.request.Request(
        url, data=data, headers={"Content-Type": "application/json"}, method="POST"
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status
    except urllib.error.HTTPError as error:
        raise RuntimeError("HTTP %s: %s" % (error.code, error.read().decode()))


def main():
    api_base, samples_path, attempt_text, host_text = sys.argv[1:5]
    attempt = int(attempt_text)
    host = int(host_text)
    posted = 0
    with open(samples_path) as stream:
        for line in stream:
            line = line.strip()
            if not line.startswith("{"):
                continue
            sample = json.loads(line)
            if sample.get("v") != 1:
                continue
            post(
                api_base + "/api/v1/ingest",
                {"sample": sample, "execution_attempt_id": attempt, "host_id": host},
            )
            posted += 1
    if posted == 0:
        raise RuntimeError("no versioned samples ingested")
    print(json.dumps({"ingested": posted}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("ingest_samples: %s" % error, file=sys.stderr)
        sys.exit(1)
