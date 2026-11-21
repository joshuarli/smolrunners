package main

import (
	"context"
	"io"
	"testing"

	"github.com/joshuarli/smolrunners/adapter/proto"
)

func TestStdioServerDrivenByFramedProtocol(t *testing.T) {
	inReader, inWriter := io.Pipe()
	outReader, outWriter := io.Pipe()

	start := proto.Frame{
		Kind:          proto.KindRun,
		SchemaVersion: proto.SchemaVersion,
		ID:            "evt-start",
		Scenario:      "job-available-to-acquire",
	}
	done := make(chan error, 1)
	go func() {
		done <- serve(context.Background(), inReader, outWriter)
		outWriter.Close()
	}()

	if err := proto.WriteFrame(inWriter, &start); err != nil {
		t.Fatal(err)
	}

	var events []string
	for {
		frame, err := proto.ReadFrame(outReader)
		if err == io.EOF {
			t.Fatal("adapter closed stdout before final frame")
		}
		if err != nil {
			t.Fatal(err)
		}
		if frame.Kind == proto.KindSession && frame.SessionEvent == "adapter_stopped" {
			break
		}
		if !frame.IsAck() {
			events = append(events, frame.Kind)
		}
		ack := proto.AckFor(frame.CorrelationID(), "accept")
		if err := proto.WriteFrame(inWriter, &ack); err != nil {
			t.Fatal(err)
		}
	}

	if err := <-done; err != nil {
		t.Fatalf("run: %v", err)
	}
	var sawOffer, sawOutcome bool
	for _, kind := range events {
		if kind == proto.KindJobAvailable {
			sawOffer = true
		}
		if kind == proto.KindAcquireOutcome {
			sawOutcome = true
		}
	}
	if !sawOffer || !sawOutcome {
		t.Fatalf("events = %v", events)
	}
	inWriter.Close()
}
