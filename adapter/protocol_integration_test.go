package main

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/joshuarli/smolrunners/adapter/proto"
)

func writeTestFile(t *testing.T, name string, data []byte, mode os.FileMode) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, data, mode); err != nil {
		t.Fatal(err)
	}
	return path
}
func send(t *testing.T, w io.Writer, f proto.Frame) {
	t.Helper()
	if err := proto.WriteFrame(w, &f); err != nil {
		t.Fatal(err)
	}
}
func receive(t *testing.T, r io.Reader) *proto.Frame {
	t.Helper()
	f, err := proto.ReadFrame(r)
	if err != nil {
		t.Fatal(err)
	}
	return f
}
func TestFakeCommandDuringEvent(t *testing.T) {
	scenario := `{"name":"commands","session":{"session_id":"11111111-1111-1111-1111-111111111111","statistics":{"total_assigned_jobs":1}},"steps":[{"kind":"message","message_id":1,"after_ms":50,"job_started":[{"request_id":1,"job_id":"j","runner_name":"$jit"}],"job_completed":[{"request_id":1,"job_id":"j","runner_name":"$jit"}]}]}`
	path := writeTestFile(t, "commands.json", []byte(scenario), 0600)
	t.Setenv("SMOLRUNNERS_SCENARIOS_DIR", filepath.Dir(path))
	ir, iw := io.Pipe()
	or, ow := io.Pipe()
	defer iw.Close()
	defer or.Close()
	done := make(chan error, 1)
	go func() { done <- serve(context.Background(), ir, ow); ow.Close() }()
	send(t, iw, proto.Frame{Kind: proto.KindRun, SchemaVersion: 1, ID: "run", Scenario: "commands"})
	event := receive(t, or)
	if event.Kind != proto.KindCapacity {
		t.Fatalf("first event %s", event.Kind)
	}
	send(t, iw, proto.Frame{Kind: proto.KindJITRequest, SchemaVersion: 1, ID: "c1", RunnerName: "runner-1"})
	result := receive(t, or)
	if result.Kind != proto.KindCommandResult || result.OK == nil || !*result.OK || result.RunnerID == nil || *result.RunnerID != 1 {
		t.Fatalf("result: %+v", result)
	}
	decoded, err := base64.StdEncoding.DecodeString(result.EncodedJITConfig)
	if err != nil {
		t.Fatal(err)
	}
	var config map[string]string
	if err = json.Unmarshal(decoded, &config); err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{"runner", "credentials", "credentials_rsaparams"} {
		b, e := base64.StdEncoding.DecodeString(config[key])
		if e != nil || !json.Valid(b) {
			t.Fatalf("invalid %s", key)
		}
	}
	runner, _ := base64.StdEncoding.DecodeString(config["runner"])
	if !strings.Contains(string(runner), "http://127.0.0.1:9") {
		t.Fatal("wrong server")
	}
	send(t, iw, proto.AckFor(event.ID, "accept"))
	names := map[string]string{}
	for {
		f := receive(t, or)
		if f.Kind == proto.KindSession && f.SessionEvent == "adapter_stopped" {
			break
		}
		if f.Kind == proto.KindJobStarted || f.Kind == proto.KindJobCompleted {
			names[f.Kind] = f.RunnerName
		}
		send(t, iw, proto.AckFor(f.ID, "accept"))
	}
	if names[proto.KindJobStarted] != "runner-1" || names[proto.KindJobCompleted] != "runner-1" {
		t.Fatalf("runner names: %v", names)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}
func TestGitHubModeCommands(t *testing.T) {
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/registration-token"):
			w.WriteHeader(201)
			fmt.Fprint(w, `{"token":"registration"}`)
		case strings.HasSuffix(r.URL.Path, "/actions/runner-registration"):
			token := "eyJhbGciOiJub25lIn0." + base64.RawURLEncoding.EncodeToString([]byte(fmt.Sprintf(`{"exp":%d}`, time.Now().Add(time.Hour).Unix()))) + "."
			w.WriteHeader(201)
			fmt.Fprintf(w, `{"url":%q,"token":%q}`, server.URL+"/tenant/123/", token)
		case strings.HasSuffix(r.URL.Path, "/runnerscalesets") && r.Method == "GET":
			fmt.Fprint(w, `{"count":0,"value":[]}`)
		case strings.HasSuffix(r.URL.Path, "/runnerscalesets") && r.Method == "POST":
			var v struct {
				RunnerSetting struct {
					DisableUpdate bool `json:"disableUpdate"`
				} `json:"runnerSetting"`
				Labels []struct {
					Name string `json:"name"`
				} `json:"labels"`
			}
			if err := json.NewDecoder(r.Body).Decode(&v); err != nil || !v.RunnerSetting.DisableUpdate || len(v.Labels) != 1 || v.Labels[0].Name != "linux" {
				t.Errorf("create request: %+v, %v", v, err)
			}
			fmt.Fprint(w, `{"id":7,"name":"set"}`)
		case strings.HasSuffix(r.URL.Path, "/7/sessions") && r.Method == "POST":
			fmt.Fprintf(w, `{"sessionId":"11111111-1111-1111-1111-111111111111","ownerName":"test","runnerScaleSet":{"id":7},"messageQueueUrl":%q,"messageQueueAccessToken":"queue","statistics":{"totalAssignedJobs":1}}`, server.URL+"/queue")
		case strings.HasSuffix(r.URL.Path, "/7/generatejitconfig"):
			var v struct {
				Name string `json:"name"`
			}
			json.NewDecoder(r.Body).Decode(&v)
			if v.Name != "test-runner" {
				t.Errorf("jit name %q", v.Name)
			}
			fmt.Fprint(w, `{"runner":{"id":42},"encodedJITConfig":"encoded"}`)
		case strings.HasSuffix(r.URL.Path, "/agents/42") && r.Method == "DELETE":
			w.WriteHeader(404)
		case strings.Contains(r.URL.Path, "/sessions/") && r.Method == "DELETE":
			w.WriteHeader(204)
		case r.URL.Path == "/queue":
			<-r.Context().Done()
		default:
			t.Errorf("unexpected request %s %s", r.Method, r.URL.Path)
			w.WriteHeader(500)
		}
	}))
	defer server.Close()
	token := writeTestFile(t, "token", []byte("secret"), 0600)
	cfg := writeTestFile(t, "config", []byte(fmt.Sprintf(`{"registration_url":%q,"scale_set_name":"set","labels":["linux"],"max_runners":3,"auth":{"token_file":%q}}`, server.URL+"/org", token)), 0600)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ir, iw := io.Pipe()
	or, ow := io.Pipe()
	defer iw.Close()
	defer or.Close()
	done := make(chan error, 1)
	go func() { done <- serveMode(ctx, ir, ow, cfg); ow.Close() }()
	send(t, iw, proto.Frame{Kind: proto.KindRun, SchemaVersion: 1, ID: "run"})
	event := receive(t, or)
	if event.Kind != proto.KindCapacity {
		t.Fatalf("expected capacity: %+v", event)
	}
	send(t, iw, proto.Frame{Kind: proto.KindJITRequest, SchemaVersion: 1, ID: "jit", RunnerName: "test-runner"})
	result := receive(t, or)
	if result.Kind != proto.KindCommandResult || result.OK == nil || !*result.OK || result.RunnerID == nil || *result.RunnerID != 42 || result.EncodedJITConfig != "encoded" {
		t.Fatalf("jit result: %+v", result)
	}
	send(t, iw, proto.Frame{Kind: proto.KindRemoveRunner, SchemaVersion: 1, ID: "remove", RunnerID: proto.Int64(42)})
	result = receive(t, or)
	if result.OK == nil || !*result.OK {
		t.Fatalf("remove result: %+v", result)
	}
	cancel()
	send(t, iw, proto.AckFor(event.ID, "accept"))
	final := receive(t, or)
	if final.SessionEvent != "adapter_stopped" {
		t.Fatalf("final: %+v", final)
	}
	<-done
}
func TestGitHubAppConfigAndSecretPermissions(t *testing.T) {
	config := writeTestFile(t, "config", []byte(`{"registration_url":"http://localhost/org","scale_set_name":"s","labels":[],"max_runners":1,"auth":{"github_app":{"client_id":"id"}}}`), 0600)
	_, err := loadConfig(config)
	if err == nil || !strings.Contains(err.Error(), "installation_id") {
		t.Fatalf("error %v", err)
	}
	secret := writeTestFile(t, "key", []byte("key"), 0644)
	_, err = readSecret(secret)
	if err == nil || !strings.Contains(err.Error(), "chmod 600") {
		t.Fatalf("error %v", err)
	}
}
