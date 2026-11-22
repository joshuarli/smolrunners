package main

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"github.com/actions/scaleset"
	"github.com/actions/scaleset/listener"
	"github.com/joshuarli/smolrunners/adapter/internal/fakebroker"
	"github.com/joshuarli/smolrunners/adapter/proto"
	"github.com/joshuarli/smolrunners/adapter/scaler"
	"io"
	"os"
	"path/filepath"
)

func scenariosDir() string {
	if dir := os.Getenv("SMOLRUNNERS_SCENARIOS_DIR"); dir != "" {
		return dir
	}
	return filepath.Join("testdata", "scenarios")
}
func fakeJIT(name string, id int64) string {
	encode := func(v any) string { b, _ := json.Marshal(v); return base64.StdEncoding.EncodeToString(b) }
	runner := map[string]any{
		"agentId":    id,
		"agentName":  name,
		"poolId":     1,
		"serverUrl":  "http://127.0.0.1:9",
		"gitHubUrl":  "http://127.0.0.1:9/smolrunners/fake",
		"workFolder": "_work",
		"ephemeral":  true,
	}
	credentials := map[string]any{
		"scheme": "OAuth",
		"data":   map[string]string{"clientId": "fake", "authorizationUrl": "http://127.0.0.1:9/token"},
	}
	return encode(map[string]string{
		".runner":                encode(runner),
		".credentials":           encode(credentials),
		".credentials_rsaparams": encode(map[string]string{}),
	})
}
func commandHandler(broker *fakebroker.Client, client *scaleset.Client, setID int) scaler.CommandHandler {
	return func(ctx context.Context, command proto.Frame) proto.Frame {
		result := proto.Frame{Kind: proto.KindCommandResult, SchemaVersion: proto.SchemaVersion, ID: "res-" + command.ID, CommandID: command.ID, OK: proto.Bool(false)}
		var err error
		switch command.Kind {
		case proto.KindJITRequest:
			if command.RunnerName == "" {
				err = errors.New("missing runner_name")
				break
			}
			if broker != nil {
				id := broker.JIT(command.RunnerName)
				result.RunnerID = proto.Int64(id)
				result.EncodedJITConfig = fakeJIT(command.RunnerName, id)
			} else {
				var jit *scaleset.RunnerScaleSetJitRunnerConfig
				jit, err = client.GenerateJitRunnerConfig(ctx, &scaleset.RunnerScaleSetJitRunnerSetting{Name: command.RunnerName, WorkFolder: "_work"}, setID)
				if err == nil {
					if jit.Runner == nil {
						err = errors.New("jit response missing runner")
					} else {
						result.RunnerID = proto.Int64(int64(jit.Runner.ID))
						result.EncodedJITConfig = jit.EncodedJITConfig
					}
				}
			}
		case proto.KindRemoveRunner:
			if command.RunnerID == nil {
				err = errors.New("missing runner_id")
				break
			}
			if broker != nil {
				broker.Remove(*command.RunnerID)
			} else {
				err = client.RemoveRunner(ctx, *command.RunnerID)
				if notFound(err) {
					err = nil
				}
			}
		default:
			err = fmt.Errorf("unknown command %q", command.Kind)
		}
		if err != nil {
			result.Error = err.Error()
		} else {
			result.OK = proto.Bool(true)
		}
		return result
	}
}
func runMode(ctx context.Context, in io.Reader, out io.Writer, configPath string) (runErr error) {
	start, err := proto.ReadFrame(in)
	if err != nil {
		return fmt.Errorf("read start frame: %w", err)
	}
	if start.Kind != proto.KindRun {
		return fmt.Errorf("expected run frame, got %q", start.Kind)
	}
	var broker *fakebroker.Client
	var client *scaleset.Client
	var session *scaleset.MessageSessionClient
	var listenerClient listener.Client
	setID, max := 1, 1000
	if configPath == "" {
		if start.Scenario == "" {
			return errors.New("expected run frame with scenario")
		}
		scenario, e := fakebroker.LoadScenario(fakebroker.ScenarioPath(scenariosDir(), start.Scenario))
		if e != nil {
			return e
		}
		broker = fakebroker.New(scenario)
		listenerClient = broker
	} else {
		c, e := loadConfig(configPath)
		if e != nil {
			return e
		}
		client, session, setID, e = c.setup(ctx)
		if e != nil {
			return e
		}
		defer session.Close(context.Background())
		max = *c.MaxRunners
		listenerClient = session
	}
	coordinator := scaler.NewFramedCoordinator(in, out)
	coordinator.Start(ctx, commandHandler(broker, client, setID))
	defer func() {
		if err := coordinator.Stop(); err != nil {
			runErr = errors.Join(runErr, fmt.Errorf("write command result: %w", err))
		}
	}()
	scale := scaler.New(listenerClient, coordinator, max)
	l, err := listener.New(listenerClient, listener.Config{ScaleSetID: setID, MaxRunners: max})
	if err != nil {
		return err
	}
	err = l.Run(ctx, scale)
	if errors.Is(err, fakebroker.ErrScenarioEnd) {
		return nil
	}
	return err
}
func serveMode(ctx context.Context, in io.Reader, out io.Writer, configPath string) error {
	err := runMode(ctx, in, out, configPath)
	frame := proto.Frame{Kind: proto.KindSession, SchemaVersion: proto.SchemaVersion, ID: "evt-final", SessionEvent: "adapter_stopped"}
	if err != nil {
		frame.Reason = err.Error()
	}
	if e := proto.WriteFrame(out, &frame); e != nil {
		return fmt.Errorf("write final frame: %w", e)
	}
	return err
}
func serve(ctx context.Context, in io.Reader, out io.Writer) error {
	return serveMode(ctx, in, out, "")
}
func main() {
	flags := flag.NewFlagSet(os.Args[0], flag.ContinueOnError)
	fake := flags.Bool("fake", false, "use fake broker")
	config := flags.String("github-config", "", "GitHub config file")
	if err := flags.Parse(os.Args[1:]); err != nil {
		os.Exit(2)
	}
	if (*fake == (*config != "")) || flags.NArg() != 0 {
		fmt.Fprintln(os.Stderr, "usage: smolrunners-adapter --fake | --github-config FILE")
		os.Exit(2)
	}
	if err := serveMode(context.Background(), os.Stdin, os.Stdout, *config); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
