package main

import (
	"os"
	"path/filepath"

	"github.com/joshuarli/smolrunners/adapter/proto"
)

func emit(dir, name string, frame proto.Frame) error {
	bytes, err := proto.EncodeFrame(&frame)
	if err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, name+".bin"), bytes, 0o644)
}

func main() {
	dir := "testdata/frames"
	if len(os.Args) > 1 {
		dir = os.Args[1]
	}
	frames := []struct {
		name  string
		frame proto.Frame
	}{
		{"run", proto.Frame{Kind: proto.KindRun, SchemaVersion: proto.SchemaVersion, ID: "evt-0000", Scenario: "statistics-only-first-message"}},
		{"capacity", proto.Frame{Kind: proto.KindCapacity, SchemaVersion: proto.SchemaVersion, ID: "evt-0001", AssignedJobs: proto.Int64(3), ReportedCapacity: proto.Int64(7)}},
		{"job_available", proto.Frame{Kind: proto.KindJobAvailable, SchemaVersion: proto.SchemaVersion, ID: "evt-0002", RequestID: proto.Int64(11), JobID: "job-abc", Labels: []string{"smol-debian13-arm64-4c8g"}}},
		{"job_assigned", proto.Frame{Kind: proto.KindJobAssigned, SchemaVersion: proto.SchemaVersion, ID: "evt-0003", RequestID: proto.Int64(11), JobID: "job-abc"}},
		{"job_started", proto.Frame{Kind: proto.KindJobStarted, SchemaVersion: proto.SchemaVersion, ID: "evt-0004", RequestID: proto.Int64(11), JobID: "job-abc", RunnerName: "runner-1"}},
		{"job_completed", proto.Frame{Kind: proto.KindJobCompleted, SchemaVersion: proto.SchemaVersion, ID: "evt-0005", RequestID: proto.Int64(11), JobID: "job-abc", Result: "success"}},
		{"acquire_outcome", proto.Frame{Kind: proto.KindAcquireOutcome, SchemaVersion: proto.SchemaVersion, ID: "evt-0006", RequestIDs: []int64{11}, AcquiredIDs: []int64{11}}},
		{"acquire_outcome_empty", proto.Frame{Kind: proto.KindAcquireOutcome, SchemaVersion: proto.SchemaVersion, ID: "evt-0007", RequestIDs: []int64{}, AcquiredIDs: []int64{}}},
		{"session", proto.Frame{Kind: proto.KindSession, SchemaVersion: proto.SchemaVersion, ID: "evt-0008", SessionEvent: "session_refresh"}},
		{"vm_delete", proto.Frame{Kind: proto.KindVmDelete, SchemaVersion: proto.SchemaVersion, ID: "evt-0009", JobID: "job-abc"}},
		{"ack", proto.Frame{Kind: proto.KindAck, SchemaVersion: proto.SchemaVersion, ID: "evt-0010", EventID: "evt-0002", Decision: "accepted", Recorded: proto.Bool(true)}},
	}
	for _, item := range frames {
		if err := emit(dir, item.name, item.frame); err != nil {
			panic(err)
		}
	}
}
