#!/bin/bash

# Set variables
CONTAINER_NAME="vecnoscan-indexer"
IMAGE_NAME="vecnoscan-indexer"
TAG="latest"

# Check if a container with the specified name already exists
container_id=$(docker ps -aq -f "name=$CONTAINER_NAME")

# If the container exists, stop and remove it
if [ -n "$container_id" ]; then
    echo "Stopping and removing existing container: $container_id"
    docker stop "$container_id"
    docker rm "$container_id"
else
    echo "No existing container found with name: $CONTAINER_NAME"
fi

# Build the Docker image
echo "Building image: $IMAGE_NAME:$TAG"
docker build -t "$IMAGE_NAME:$TAG" .

# Run the container from the new image
echo "Running new container: $CONTAINER_NAME"
docker run --network host --restart unless-stopped --name "$CONTAINER_NAME" -d "$IMAGE_NAME:$TAG" 

# Check if a container with the specified name already exists
container_id=$(docker ps -aq -f "name=$CONTAINER_NAME")
docker logs -f "$container_id"