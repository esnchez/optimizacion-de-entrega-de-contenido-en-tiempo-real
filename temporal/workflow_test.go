package main

import (
	"errors"
	"testing"
	"time"

	"github.com/stretchr/testify/mock"
	"github.com/stretchr/testify/require"
	"go.temporal.io/sdk/testsuite"
)

// El entorno de pruebas de Temporal ejecuta el workflow en memoria: las activities se sustituyen
// por mocks y los temporizadores (workflow.Sleep) se saltan al instante.

var input = PrescaleInput{Namespace: "cdn", HPA: "content-delivery", Deployment: "content-delivery", MinReplicas: 4, HoldSeconds: 600}

var acquired = AcquireResult{Acquired: true, OriginalMin: 2}

func TestRaisesWaitsHoldsAndRestores(t *testing.T) {
	var ts testsuite.WorkflowTestSuite
	env := ts.NewTestWorkflowEnvironment()
	a := &Activities{}
	env.OnActivity(a.AcquirePrescale, mock.Anything, "cdn", "content-delivery", mock.Anything, mock.Anything).Return(acquired, nil)
	env.OnActivity(a.SetMinReplicas, mock.Anything, "cdn", "content-delivery", int32(4)).Return(nil).Once()
	env.OnActivity(a.WaitForReadyReplicas, mock.Anything, "cdn", "content-delivery", int32(4)).Return(nil)
	env.OnActivity(a.RestoreAndRelease, mock.Anything, "cdn", "content-delivery", mock.Anything, int32(2)).Return(nil).Once()

	env.ExecuteWorkflow(PrescaleForEvent, input)

	require.True(t, env.IsWorkflowCompleted())
	require.NoError(t, env.GetWorkflowError())
	var result PrescaleResult
	require.NoError(t, env.GetWorkflowResult(&result))
	require.Equal(t, PrescaleResult{OriginalMinReplicas: 2, RaisedTo: 4, HeldSeconds: 600}, result)
	env.AssertExpectations(t) // el mínimo se subió a 4 y se restauró a 2
}

func TestLeaseCoversTheHoldTime(t *testing.T) {
	var ts testsuite.WorkflowTestSuite
	env := ts.NewTestWorkflowEnvironment()
	a := &Activities{}
	// El lease debe durar la espera más el margen: si caducara antes, otra ejecución podría entrar.
	env.OnActivity(a.AcquirePrescale, mock.Anything, mock.Anything, mock.Anything, mock.Anything, 600*time.Second+leaseMargin).
		Return(acquired, nil).Once()
	env.OnActivity(a.SetMinReplicas, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(nil)
	env.OnActivity(a.WaitForReadyReplicas, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(nil)
	env.OnActivity(a.RestoreAndRelease, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(nil)

	env.ExecuteWorkflow(PrescaleForEvent, input)

	require.NoError(t, env.GetWorkflowError())
	env.AssertExpectations(t)
}

func TestSkipsWithoutTouchingTheHPAWhenAnotherRunHoldsTheLease(t *testing.T) {
	var ts testsuite.WorkflowTestSuite
	env := ts.NewTestWorkflowEnvironment()
	a := &Activities{}
	env.OnActivity(a.AcquirePrescale, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything).
		Return(AcquireResult{Acquired: false, HeldBy: "prescale-other/run", HeldUntil: "2026-11-01T20:00:00Z"}, nil)

	env.ExecuteWorkflow(PrescaleForEvent, input)

	// Termina bien (no es un fallo: no dispara la alerta TemporalWorkflowFailed) y no cambia nada.
	require.NoError(t, env.GetWorkflowError())
	var result PrescaleResult
	require.NoError(t, env.GetWorkflowResult(&result))
	require.True(t, result.Skipped)
	require.Contains(t, result.SkippedBecause, "prescale-other/run")
	env.AssertNotCalled(t, "SetMinReplicas", mock.Anything, mock.Anything, mock.Anything, mock.Anything)
	env.AssertNotCalled(t, "RestoreAndRelease", mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything)
}

func TestRetriesUntilPodsAreReady(t *testing.T) {
	var ts testsuite.WorkflowTestSuite
	env := ts.NewTestWorkflowEnvironment()
	a := &Activities{}
	env.OnActivity(a.AcquirePrescale, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(acquired, nil)
	env.OnActivity(a.SetMinReplicas, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(nil)
	env.OnActivity(a.RestoreAndRelease, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(nil)
	// Dos intentos fallidos y luego éxito: Temporal reintenta según la RetryPolicy del workflow.
	env.OnActivity(a.WaitForReadyReplicas, mock.Anything, mock.Anything, mock.Anything, mock.Anything).
		Return(errors.New("2 of 4 pods ready")).Twice()
	env.OnActivity(a.WaitForReadyReplicas, mock.Anything, mock.Anything, mock.Anything, mock.Anything).
		Return(nil).Once()

	env.ExecuteWorkflow(PrescaleForEvent, input)

	require.NoError(t, env.GetWorkflowError())
	env.AssertNumberOfCalls(t, "WaitForReadyReplicas", 3)
}

func TestRestoresTheMinimumEvenWhenPodsNeverBecomeReady(t *testing.T) {
	var ts testsuite.WorkflowTestSuite
	env := ts.NewTestWorkflowEnvironment()
	a := &Activities{}
	env.OnActivity(a.AcquirePrescale, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(acquired, nil)
	env.OnActivity(a.SetMinReplicas, mock.Anything, "cdn", "content-delivery", int32(4)).Return(nil).Once()
	env.OnActivity(a.WaitForReadyReplicas, mock.Anything, mock.Anything, mock.Anything, mock.Anything).
		Return(errors.New("0 of 4 pods ready"))
	env.OnActivity(a.RestoreAndRelease, mock.Anything, "cdn", "content-delivery", mock.Anything, int32(2)).Return(nil).Once()

	env.ExecuteWorkflow(PrescaleForEvent, input)

	require.Error(t, env.GetWorkflowError()) // se agotan los 24 intentos
	env.AssertExpectations(t)                // pero el mínimo vuelve a 2 y se libera el lease
}

func TestLeaseHolder(t *testing.T) {
	now := time.Date(2026, 11, 1, 19, 0, 0, 0, time.UTC)
	lease := func(owner, until string) map[string]string {
		return map[string]string{annotationOwner: owner, annotationUntil: until}
	}
	cases := []struct {
		name        string
		annotations map[string]string
		heldByOther bool
	}{
		{"no lease", map[string]string{}, false},
		{"own lease (retry of a successful acquire)", lease("me", "2026-11-01T20:00:00Z"), false},
		{"valid lease of another run", lease("other", "2026-11-01T20:00:00Z"), true},
		{"expired lease of another run", lease("other", "2026-11-01T18:00:00Z"), false},
		{"unreadable expiry", lease("other", "not-a-date"), false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			heldByOther, _, _ := leaseHolder(c.annotations, "me", now)
			require.Equal(t, c.heldByOther, heldByOther)
		})
	}
}
