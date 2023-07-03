#
# A script to get your liked songs and create a new playlist with unbiased
# shuffle.
#
# By Joel Odom (joelodom@gmail.com) who can help you with the usage.
#
# Thanks ChatGPT for help in buddy coding this.
#


import sys
import spotipy
from spotipy.oauth2 import SpotifyOAuth
import requests
import random

# Check if client_id and client_secret were provided as command-line arguments
if len(sys.argv) < 3:
    print("Usage: python script.py <client_id> <client_secret>")
    sys.exit(1)

# Retrieve the client_id and client_secret from command-line arguments
client_id = sys.argv[1]
client_secret = sys.argv[2]
redirect_uri = 'http://localhost:3000'  # This should match your Spotify application's redirect URI

# Authorization scope (add any additional scopes as needed)
scope = 'user-library-read playlist-modify-private'

try:
    # Create a Spotify API client
    sp = spotipy.Spotify(auth_manager=SpotifyOAuth(client_id=client_id,
                                                   client_secret=client_secret,
                                                   redirect_uri=redirect_uri,
                                                   scope=scope))
    
    # Retrieve all liked songs in batches of fifty (which I think is the limit)
    limit = 50  # Number of songs to retrieve per batch
    offset = 0  # Initial offset
    liked_songs = []

    while True:
        tracks = sp.current_user_saved_tracks(limit=limit, offset=offset)

        # Break the loop if no more tracks are returned
        if len(tracks['items']) == 0:
            break

        liked_songs.extend(tracks['items'])
        offset += limit
    
    # Extract the URIs of the liked songs
    song_uris = [track['track']['uri'] for track in liked_songs]

    # Shuffle the song URIs
    random.shuffle(song_uris)

    # Create a new playlist
    playlist_name = 'Shuffled Liked Songs'  # Change the playlist name as desired
    playlist_description = 'Playlist of shuffled liked songs'

    user_id = sp.current_user()['id']
    playlist = sp.user_playlist_create(
        user_id, playlist_name, public=False, description=playlist_description)

    # Add the shuffled songs to the playlist in batches
    batch_size = 100  # Number of songs to add per batch (I think this is the limit)
    total_songs = len(song_uris)

    for i in range(0, total_songs, batch_size):
        batch_songs = song_uris[i:i + batch_size]
        sp.playlist_add_items(playlist['id'], batch_songs)

    print(f'Playlist "{playlist_name}" created with {total_songs} songs!')

except spotipy.SpotifyException as e:
    print("An error occurred with the Spotify API:")
    print(e)

except requests.exceptions.RequestException as e:
    print("A network connection error occurred:")
    print(e)
