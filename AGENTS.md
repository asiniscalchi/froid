# AGENTS.md

## Foundational Rule
- this file defines mandatory agent behavior and takes precedence over default agent workflows unless overridden by higher-priority instructions
- see `CONTRIBUTING.md` for project-specific conventions and setup.

## Clarification
- if the requested feature, expected behavior, or acceptance criteria are unclear, ask for clarification before planning or implementation
- do not make hidden assumptions about product behavior, API contracts, or user intent when clarification is required

## Planning
- inspect the relevant existing code before making changes
- for every non-trivial feature, bug fix, or refactoring, create a concrete step-by-step plan before implementation
- make each plan step a meaningful, reviewable unit of work that contributes directly to the final goal
- execute one plan step at a time
- if new complexity, constraints, or scope changes appear, update the plan explicitly before continuing

## Implementation
- keep changes small, focused, and scoped to the requested work
- prefer modifying existing code over rewriting working code
- preserve existing behavior unless the task explicitly requires changing it
- avoid unrelated refactors during feature or bug-fix work unless they are necessary to complete the task safely
- keep public interfaces stable unless a change is explicitly required

## Code Quality
- write idiomatic code that follows the natural patterns of the language, framework, and repository
- prefer clear, simple, and maintainable solutions over clever, dense, or overly abstract ones
- avoid overengineering, speculative generalization, and premature abstraction
- when refactoring, make the result more idiomatic and maintainable, not merely different
- do not introduce unnecessary dependencies, frameworks, or architectural patterns without explicit approval
- prefer consistent naming, structure, and control flow that match the surrounding codebase

## Dependency and Library Usage
- if the required usage is unclear, non-obvious, version-sensitive, or not already demonstrated in the codebase, verify it against official documentation before implementing
- do not guess library APIs, configuration, or framework behavior when the correct usage can be confirmed from official documentation
- when using unfamiliar features, base the implementation on the official documentation rather than memory alone

## Testing
- cover all new or changed behavior with automated tests
- prefer unit tests when appropriate, but use the most suitable test level for the behavior being changed
- for bug fixes, first add or update a test that reproduces the bug when practical, then implement the fix
- do not consider work complete until the relevant tests pass
- do not claim something works unless it has been verified

## Validation
- before pushing, run locally all checks required by CI for the affected codebase
- local results must be green for the same validation expected in CI before pushing
- do not push code that is known to fail formatting, linting, type checks, tests, or any other required validation
- if a check cannot be run locally, state that explicitly

## Git Workflow
- use a dedicated branch for each feature, bug fix, or refactoring
- never push directly to main
- keep commits ordered, coherent, and reviewable
- do not mix unrelated changes in the same commit

## Commit Discipline
- after completing each plan step, create one git commit corresponding to that step
- each commit must reflect a completed, meaningful unit of work
- commit messages must clearly describe the change introduced by that step
- do not create trivial or artificial commits just to satisfy the process

## Communication
- be explicit about assumptions, tradeoffs, risks, and anything not verified
- do not present guesses as facts
- state clearly what was changed, what was tested, and any remaining limitations
- when blocked by ambiguity, ask the smallest necessary clarifying question

## Done Criteria
- the request is understood
- the plan was created before implementation and kept up to date
- each meaningful plan step was committed separately
- the implementation is complete
- the code is idiomatic for the language, framework, and repository
- refactorings make the code more idiomatic and maintainable
- relevant automated tests were added or updated
- all required local CI-equivalent checks are green
- the branch is ready for review
- the pull request is created when possible
