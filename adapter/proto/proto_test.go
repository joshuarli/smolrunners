package proto

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func goldenDir(t *testing.T) string {
	t.Helper()
	return filepath.Join("..", "testdata", "frames")
}

func TestGoldenFramesRoundTrip(t *testing.T) {
	entries, err := os.ReadDir(goldenDir(t))
	if err != nil {
		t.Fatalf("read golden frames: %v", err)
	}
	if len(entries) == 0 {
		t.Fatal("no golden frames")
	}
	for _, entry := range entries {
		if entry.IsDir() || filepath.Ext(entry.Name()) != ".bin" {
			continue
		}
		t.Run(entry.Name(), func(t *testing.T) {
			want, err := os.ReadFile(filepath.Join(goldenDir(t), entry.Name()))
			if err != nil {
				t.Fatal(err)
			}
			frame, err := DecodeFrame(want)
			if err != nil {
				t.Fatalf("decode golden frame: %v", err)
			}
			got, err := EncodeFrame(frame)
			if err != nil {
				t.Fatalf("encode golden frame: %v", err)
			}
			if !bytes.Equal(got, want) {
				t.Fatalf("round-trip mismatch\n got: %s\nwant: %s", got, want)
			}
		})
	}
}

func TestGoldenFramesExpectedKinds(t *testing.T) {
	for name, want := range map[string]string{
		"capacity.bin":       KindCapacity,
		"job_available.bin":  KindJobAvailable,
		"job_assigned.bin":   KindJobAssigned,
		"acquire_outcome.bin": KindAcquireOutcome,
		"session.bin":        KindSession,
		"ack.bin":            KindAck,
		"job_completed.bin":  KindJobCompleted,
		"vm_delete.bin":      KindVmDelete,
	} {
		body, err := os.ReadFile(filepath.Join(goldenDir(t), name))
		if err != nil {
			t.Fatal(err)
		}
		frame, err := DecodeFrame(body)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if frame.Kind != want {
			t.Errorf("%s: kind = %q, want %q", name, frame.Kind, want)
		}
		if frame.SchemaVersion != SchemaVersion {
			t.Errorf("%s: schema_version = %d", name, frame.SchemaVersion)
		}
	}
}

func TestFrameSizeLimit(t *testing.T) {
	f := &Frame{
		Kind:          KindSession,
		SchemaVersion: SchemaVersion,
		ID:            "evt-big",
		SessionEvent:  string(bytes.Repeat([]byte("x"), MaxFrameBytes)),
	}
	if _, err := EncodeFrame(f); err == nil {
		t.Fatal("oversize frame encoded without error")
	}

	var badLen [4]byte
	badLen[3] = 0xFF
	badLen[2] = 0xFF
	badLen[1] = 0xFF
	if _, err := DecodeFrame(badLen[:]); err == nil {
		t.Fatal("oversize prefix decoded without error")
	}
}

func TestFrameRejectsBadSchemaAndPrefix(t *testing.T) {
	if _, err := DecodeFrame([]byte{0, 0}); err == nil {
		t.Fatal("short prefix accepted")
	}
	body, err := os.ReadFile(filepath.Join(goldenDir(t), "capacity.bin"))
	if err != nil {
		t.Fatal(err)
	}
	frame, err := DecodeFrame(body)
	if err != nil {
		t.Fatal(err)
	}
	frame.SchemaVersion = 2
	bad, err := marshalUnchecked(frame)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := DecodeFrame(bad); err == nil {
		t.Fatal("bad schema_version accepted")
	}
}

func marshalUnchecked(f *Frame) ([]byte, error) {
	body, err := json.Marshal(f)
	if err != nil {
		return nil, err
	}
	out := make([]byte, 4+len(body))
	binary.BigEndian.PutUint32(out[:4], uint32(len(body)))
	copy(out[4:], body)
	return out, nil
}

func TestWriteReadFrameStream(t *testing.T) {
	a := &Frame{Kind: KindCapacity, SchemaVersion: SchemaVersion, ID: "evt-stream", AssignedJobs: Int64(2), ReportedCapacity: Int64(2)}
	b := &Frame{Kind: KindAck, SchemaVersion: SchemaVersion, ID: "ack-stream", EventID: "evt-stream", Decision: "accept", Recorded: Bool(true)}
	var buf bytes.Buffer
	if err := WriteFrame(&buf, a); err != nil {
		t.Fatal(err)
	}
	if err := WriteFrame(&buf, b); err != nil {
		t.Fatal(err)
	}
	gotA, err := ReadFrame(&buf)
	if err != nil {
		t.Fatal(err)
	}
	gotB, err := ReadFrame(&buf)
	if err != nil {
		t.Fatal(err)
	}
	if gotA.ID != a.ID || gotB.CorrelationID() != "evt-stream" {
		t.Fatalf("stream decode mismatch: %+v, %+v", gotA, gotB)
	}
}
