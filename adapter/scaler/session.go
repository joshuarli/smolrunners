package scaler

import (
	"context"
	"math/rand"
	"time"

	"github.com/joshuarli/smolrunners/adapter/proto"
)

const (
	SessionKeyRotated        = "key_rotated"
	SessionInstallationRevoked = "installation_revoked"
	SessionRetryGuidance     = "retry_guidance"
	SessionRefreshFailed     = "refresh_failed"
	SessionRecovered         = "session_recovered"
)

type SessionEvent struct {
	Kind           string
	RetryAfter     time.Duration
	InstallationID string
	KeyID          string
}

type SessionAuth struct {
	KeyID          string
	InstallationID string
	Revoked        bool
}

type RetryGuidance struct {
	RetryAfter time.Duration
	Retryable  bool
}

func ParseRetryGuidance(event SessionEvent) RetryGuidance {
	switch event.Kind {
	case SessionRetryGuidance, SessionRefreshFailed:
		return RetryGuidance{RetryAfter: event.RetryAfter, Retryable: true}
	case SessionInstallationRevoked:
		return RetryGuidance{Retryable: false}
	default:
		return RetryGuidance{}
	}
}

func RecoveryDelay(attempt int, rng *rand.Rand) time.Duration {
	if attempt < 0 {
		attempt = 0
	}
	base := time.Second
	for i := 0; i < attempt && base < 60*time.Second; i++ {
		base *= 2
	}
	if base > 60*time.Second {
		base = 60 * time.Second
	}
	span := int64(base / 4)
	jitter := int64(0)
	if rng != nil && span > 0 {
		jitter = rng.Int63n(2*span+1) - span
	}
	d := base + time.Duration(jitter)
	if d < time.Second {
		d = time.Second
	}
	if d > 60*time.Second {
		d = 60 * time.Second
	}
	return d
}

func (s *Scaler) ReportSession(ctx context.Context, event SessionEvent) error {
	s.mu.Lock()
	switch event.Kind {
	case SessionKeyRotated:
		if event.KeyID != "" {
			s.auth.KeyID = event.KeyID
		}
		s.auth.Revoked = false
	case SessionInstallationRevoked:
		s.auth.InstallationID = event.InstallationID
		s.auth.Revoked = true
	case SessionRecovered:
		s.auth.Revoked = false
	}
	s.mu.Unlock()

	frame := proto.Frame{
		Kind:          proto.KindSession,
		SchemaVersion: proto.SchemaVersion,
		ID:            s.ids.next("session"),
		SessionEvent:  event.Kind,
	}
	if event.Kind == SessionRetryGuidance || event.Kind == SessionRefreshFailed {
		ms := event.RetryAfter.Milliseconds()
		frame.RetryAfterMs = proto.Int64(ms)
	}
	if event.InstallationID != "" {
		frame.Reason = event.InstallationID
	}
	_, err := s.coordinator.Submit(ctx, frame)
	return err
}

func (s *Scaler) Auth() SessionAuth {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.auth
}
