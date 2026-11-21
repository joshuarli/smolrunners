package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/actions/scaleset/listener"
	"github.com/joshuarli/smolrunners/adapter/internal/fakebroker"
	"github.com/joshuarli/smolrunners/adapter/proto"
	"github.com/joshuarli/smolrunners/adapter/scaler"
)

func scenariosDir() string {
	if dir := os.Getenv("SMOLRUNNERS_SCENARIOS_DIR"); dir != "" {
		return dir
	}
	return filepath.Join("testdata", "scenarios")
}

func run(ctx context.Context, in io.Reader, out io.Writer) error {
	start, err := proto.ReadFrame(in)
	if err != nil {
		return fmt.Errorf("read start frame: %w", err)
	}
	if start.Kind != proto.KindRun || start.Scenario == "" {
		return fmt.Errorf("expected run frame with scenario, got %q", start.Kind)
	}

	scenario, err := fakebroker.LoadScenario(fakebroker.ScenarioPath(scenariosDir(), start.Scenario))
	if err != nil {
		return err
	}
	broker := fakebroker.New(scenario)
	coordinator := scaler.NewFramedCoordinator(in, out)
	scale := scaler.New(broker, coordinator, 1000)
	l, err := listener.New(broker, listener.Config{ScaleSetID: 1, MaxRunners: 1000})
	if err != nil {
		return err
	}

	err = l.Run(ctx, scale)
	if err != nil && !errors.Is(err, fakebroker.ErrScenarioEnd) {
		return err
	}
	return nil
}

func serve(ctx context.Context, in io.Reader, out io.Writer) error {
	err := run(ctx, in, out)
	frame := proto.Frame{
		Kind:          proto.KindSession,
		SchemaVersion: proto.SchemaVersion,
		ID:            "evt-final",
		SessionEvent:  "adapter_stopped",
	}
	if err != nil {
		frame.Reason = err.Error()
	}
	if writeErr := proto.WriteFrame(out, &frame); writeErr != nil {
		return fmt.Errorf("write final frame: %w", writeErr)
	}
	return err
}

func main() {
	if err := serve(context.Background(), os.Stdin, os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
