# Reviews and install tests

One folder per release. Each holds the automated code reviews of the change since the release before (`code-review-<round>.md`, in order: each round after the first reviews the change with the fixes for the round before) and the clean-install test of the release (`install-test.md`), as written, with internal references such as build machine paths removed. What a review found was fixed before the release; what an install test found went into the next release.

| release | code reviews | install test |
|---|---|---|
| [0.0.1](0.0.1/) | [the 0.0.1 change](0.0.1/code-review.md), reviewed after its release: fit once its one major finding was fixed, which 0.0.2 did | [README steps run on the release](0.0.1/install-test.md): pass |
| [0.0.2](0.0.2/) | [1](0.0.2/code-review-1.md) not fit until one finding was fixed; [2](0.0.2/code-review-2.md), [3](0.0.2/code-review-3.md), [4](0.0.2/code-review-4.md) fit | [pass](0.0.2/install-test.md), 11 findings for 0.0.3 |
| [0.0.3](0.0.3/) | [1](0.0.3/code-review-1.md) not fit (`read_image` can send data out; fixed); [2](0.0.3/code-review-2.md), [3](0.0.3/code-review-3.md) fit | [pass](0.0.3/install-test.md), 11 findings for 0.0.4 |
