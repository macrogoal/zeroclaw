#!/usr/bin/env bash
# Publish mormclaw Docker image using DOCKER_SPACE_SORA (login/namespace) and DOCKER_TOKEN_SORA (API token).
# Usage: bash docker-publish.sh [tag]
#   If no tag given: uses git describe (version from latest tag + commit, e.g. 0.1.2 or 0.1.2-1-gabc1234)
# Example: DOCKER_SPACE_SORA=myuser DOCKER_TOKEN_SORA=xxx bash docker-publish.sh
# Example: DOCKER_SPACE_SORA=myuser DOCKER_TOKEN_SORA=xxx bash docker-publish.sh 0.1.2
#
# Env vars:
#   DOCKER_SPACE_SORA - Docker Hub username or registry path (e.g. myuser or ghcr.io/myorg)
#   DOCKER_TOKEN_SORA  - Docker API token / deployment key for authentication

set -e

if [[ -n "${1}" ]]; then
  TAG="${1}"
else
  TAG="$(git describe --tags --always 2>/dev/null || echo 'latest')"
fi
IMAGE_NAME="zeroclaw"

if [[ -z "${DOCKER_SPACE_SORA}" || -z "${DOCKER_TOKEN_SORA}" ]]; then
  echo "Error: DOCKER_SPACE_SORA and DOCKER_TOKEN_SORA must be set."
  echo "  DOCKER_SPACE_SORA = Docker registry username or namespace (e.g. myuser or ghcr.io/myorg)"
  echo "  DOCKER_TOKEN_SORA  = Docker API token / deployment key"
  exit 1
fi

echo "Building ${IMAGE_NAME}:${TAG}..."
docker build -f mormsodockerfile --target release -t "${IMAGE_NAME}:${TAG}" .

REMOTE_IMAGE="${DOCKER_SPACE_SORA}/${IMAGE_NAME}:${TAG}"
echo "Logging in and pushing ${REMOTE_IMAGE}..."

# Docker Hub: login with username. GHCR/other: if namespace has registry prefix, login to that registry.
if [[ "${DOCKER_SPACE_SORA}" == *"/"* ]]; then
  REGISTRY="${DOCKER_SPACE_SORA%%/*}"
  echo "${DOCKER_TOKEN_SORA}" | docker login "${REGISTRY}" -u "${DOCKER_SPACE_SORA#*/}" --password-stdin
else
  echo "${DOCKER_TOKEN_SORA}" | docker login -u "${DOCKER_SPACE_SORA}" --password-stdin
fi

docker tag "${IMAGE_NAME}:${TAG}" "${REMOTE_IMAGE}"
docker push "${REMOTE_IMAGE}"

REMOTE_LATEST="${DOCKER_SPACE_SORA}/${IMAGE_NAME}:latest"
docker tag "${IMAGE_NAME}:${TAG}" "${REMOTE_LATEST}"
docker push "${REMOTE_LATEST}"

echo "Published ${REMOTE_IMAGE} and ${REMOTE_LATEST}"
echo "https://hub.docker.com/r/sorajez/zeroclaw"
