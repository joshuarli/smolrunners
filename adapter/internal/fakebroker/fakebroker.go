package fakebroker

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/actions/scaleset"
	"github.com/google/uuid"
)

type Statistics struct {
	TotalAvailableJobs     int `json:"total_available_jobs"`
	TotalAcquiredJobs      int `json:"total_acquired_jobs"`
	TotalAssignedJobs      int `json:"total_assigned_jobs"`
	TotalRunningJobs       int `json:"total_running_jobs"`
	TotalRegisteredRunners int `json:"total_registered_runners"`
	TotalBusyRunners       int `json:"total_busy_runners"`
	TotalIdleRunners       int `json:"total_idle_runners"`
}

func (s Statistics) toSDK() *scaleset.RunnerScaleSetStatistic {
	return &scaleset.RunnerScaleSetStatistic{
		TotalAvailableJobs:     s.TotalAvailableJobs,
		TotalAcquiredJobs:      s.TotalAcquiredJobs,
		TotalAssignedJobs:      s.TotalAssignedJobs,
		TotalRunningJobs:       s.TotalRunningJobs,
		TotalRegisteredRunners: s.TotalRegisteredRunners,
		TotalBusyRunners:       s.TotalBusyRunners,
		TotalIdleRunners:       s.TotalIdleRunners,
	}
}

type Session struct {
	SessionID  string      `json:"session_id"`
	OwnerName  string      `json:"owner_name"`
	Statistics *Statistics `json:"statistics"`
}

type Job struct {
	RunnerName         string   `json:"runner_name"`
	Result             string   `json:"result"`
	RequestID          int64    `json:"request_id"`
	JobID              string   `json:"job_id"`
	Labels             []string `json:"labels"`
	WorkflowRunID      int64    `json:"workflow_run_id"`
	OwnerName          string   `json:"owner_name"`
	RepositoryName     string   `json:"repository_name"`
	ScaleSetAssignTime string   `json:"scale_set_assign_time"`
}

func (j Job) base() scaleset.JobMessageBase {
	assignTime := time.Time{}
	if j.ScaleSetAssignTime != "" {
		if parsed, err := time.Parse(time.RFC3339, j.ScaleSetAssignTime); err == nil {
			assignTime = parsed
		}
	}
	return scaleset.JobMessageBase{
		RunnerRequestID:    j.RequestID,
		RepositoryName:     j.RepositoryName,
		OwnerName:          j.OwnerName,
		JobID:              j.JobID,
		WorkflowRunID:      j.WorkflowRunID,
		RequestLabels:      append([]string(nil), j.Labels...),
		QueueTime:          assignTime,
		ScaleSetAssignTime: assignTime,
	}
}

type Step struct {
	AfterMs    int         `json:"after_ms"`
	Kind       string      `json:"kind"`
	MessageID  int         `json:"message_id"`
	Statistics *Statistics `json:"statistics"`
	Available  []Job       `json:"job_available"`
	Assigned   []Job       `json:"job_assigned"`
	Started    []Job       `json:"job_started"`
	Completed  []Job       `json:"job_completed"`
	Error      string      `json:"error"`
}

type Scenario struct {
	Name                string    `json:"name"`
	Session             Session   `json:"session"`
	MaxMessagesPerBatch int       `json:"max_messages_per_batch"`
	AcquireResults      [][]int64 `json:"acquire_results"`
	Steps               []Step    `json:"steps"`
	AfterEnd            string    `json:"after_end"`
}

func LoadScenario(path string) (*Scenario, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("fakebroker: read scenario: %w", err)
	}
	return ParseScenario(data)
}

func ParseScenario(data []byte) (*Scenario, error) {
	var scenario Scenario
	if err := json.Unmarshal(data, &scenario); err != nil {
		return nil, fmt.Errorf("fakebroker: parse scenario: %w", err)
	}
	if scenario.Name == "" {
		return nil, errors.New("fakebroker: scenario has no name")
	}
	return &scenario, nil
}

var ErrScenarioEnd = errors.New("fakebroker: scenario ended")

type Client struct {
	scenario *Scenario

	mu           sync.Mutex
	stepIndex    int
	messages     []*scaleset.RunnerScaleSetMessage
	errors       []error
	delays       []time.Duration
	jitName      string
	runnerID     int64
	removed      []int64
	acquireIndex int
	deleted      []int
	acquireCalls [][]int64
}

func New(scenario *Scenario) *Client {
	client := &Client{scenario: scenario}
	client.expandSteps()
	return client
}

func (c *Client) expandSteps() {
	for _, step := range c.scenario.Steps {
		before := len(c.messages)
		switch step.Kind {
		case "error":
			c.errors = append(c.errors, errors.New(step.Error))
			c.messages = append(c.messages, nil)
		case "nil":
			c.errors = append(c.errors, nil)
			c.messages = append(c.messages, nil)
		default:
			batches := c.batchCount(step)
			limit := c.scenario.MaxMessagesPerBatch
			for batch := 0; batch < batches; batch++ {
				c.errors = append(c.errors, nil)
				c.messages = append(c.messages, buildMessageWithLimit(step, batch, limit))
			}
		}
		for i := before; i < len(c.messages); i++ {
			if i == before {
				c.delays = append(c.delays, time.Duration(step.AfterMs)*time.Millisecond)
			} else {
				c.delays = append(c.delays, 0)
			}
		}
	}
}

func (c *Client) batchCount(step Step) int {
	limit := c.scenario.MaxMessagesPerBatch
	if limit <= 0 {
		return 1
	}
	count := 1
	for _, n := range []int{len(step.Available), len(step.Assigned), len(step.Started), len(step.Completed)} {
		batches := (n + limit - 1) / limit
		if batches > count {
			count = batches
		}
	}
	return count
}

func buildMessageWithLimit(step Step, batch, limit int) *scaleset.RunnerScaleSetMessage {
	message := &scaleset.RunnerScaleSetMessage{
		MessageID:  step.MessageID + batch,
		Statistics: nil,
	}
	if step.Statistics != nil {
		message.Statistics = step.Statistics.toSDK()
	}
	slice := func(jobs []Job) []Job {
		if limit <= 0 {
			return jobs
		}
		start := batch * limit
		if start >= len(jobs) {
			return nil
		}
		end := start + limit
		if end > len(jobs) {
			end = len(jobs)
		}
		return jobs[start:end]
	}
	for _, job := range slice(step.Available) {
		available := &scaleset.JobAvailable{JobMessageBase: job.base()}
		message.JobAvailableMessages = append(message.JobAvailableMessages, available)
	}
	for _, job := range slice(step.Assigned) {
		message.JobAssignedMessages = append(message.JobAssignedMessages, &scaleset.JobAssigned{JobMessageBase: job.base()})
	}
	for _, job := range slice(step.Started) {
		message.JobStartedMessages = append(message.JobStartedMessages, &scaleset.JobStarted{JobMessageBase: job.base(), RunnerName: job.RunnerName})
	}
	for _, job := range slice(step.Completed) {
		message.JobCompletedMessages = append(message.JobCompletedMessages, &scaleset.JobCompleted{JobMessageBase: job.base(), RunnerName: job.RunnerName, Result: job.Result})
	}
	return message
}

func (c *Client) Session() scaleset.RunnerScaleSetSession {
	sessionID := uuid.Nil
	if c.scenario.Session.SessionID != "" {
		if parsed, err := uuid.Parse(c.scenario.Session.SessionID); err == nil {
			sessionID = parsed
		} else {
			sessionID = uuid.New()
		}
	} else {
		sessionID = uuid.New()
	}
	var statistics *scaleset.RunnerScaleSetStatistic
	if c.scenario.Session.Statistics != nil {
		statistics = c.scenario.Session.Statistics.toSDK()
	}
	return scaleset.RunnerScaleSetSession{
		SessionID:      sessionID,
		OwnerName:      c.scenario.Session.OwnerName,
		RunnerScaleSet: &scaleset.RunnerScaleSet{},
		Statistics:     statistics,
	}
}

func (c *Client) GetMessage(ctx context.Context, lastMessageID, maxCapacity int) (*scaleset.RunnerScaleSetMessage, error) {
	c.mu.Lock()
	if c.stepIndex >= len(c.messages) {
		c.mu.Unlock()
		if c.scenario.AfterEnd == "error" {
			return nil, errors.New("fakebroker: session refresh failed")
		}
		return nil, ErrScenarioEnd
	}
	index := c.stepIndex
	c.stepIndex++
	delay := c.delays[index]
	c.mu.Unlock()
	if delay > 0 {
		timer := time.NewTimer(delay)
		defer timer.Stop()
		select {
		case <-timer.C:
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.errors[index] != nil {
		return nil, c.errors[index]
	}
	message := c.messages[index]
	if message != nil {
		for _, job := range message.JobStartedMessages {
			if job.RunnerName == "$jit" {
				job.RunnerName = c.jitName
			}
		}
		for _, job := range message.JobCompletedMessages {
			if job.RunnerName == "$jit" {
				job.RunnerName = c.jitName
			}
		}
	}
	return message, nil
}

func (c *Client) JIT(name string) int64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.runnerID++
	c.jitName = name
	return c.runnerID
}
func (c *Client) Remove(id int64) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.removed = append(c.removed, id)
}

func (c *Client) DeleteMessage(ctx context.Context, messageID int) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.deleted = append(c.deleted, messageID)
	return nil
}

func (c *Client) AcquireJobs(ctx context.Context, requestIDs []int64) ([]int64, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.acquireCalls = append(c.acquireCalls, append([]int64(nil), requestIDs...))
	if c.acquireIndex < len(c.scenario.AcquireResults) {
		result := c.scenario.AcquireResults[c.acquireIndex]
		c.acquireIndex++
		return append([]int64(nil), result...), nil
	}
	return nil, nil
}

func (c *Client) DeletedMessages() []int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]int(nil), c.deleted...)
}

func (c *Client) AcquireCalls() [][]int64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([][]int64, 0, len(c.acquireCalls))
	for _, call := range c.acquireCalls {
		out = append(out, append([]int64(nil), call...))
	}
	return out
}

func ScenarioPath(scenariosDir, name string) string {
	return filepath.Join(scenariosDir, name+".json")
}
