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
    
    # Get user's liked songs
    liked_songs = sp.current_user_saved_tracks(limit=20)  # Adjust the limit as needed

    # Extract the URIs of the liked songs
    song_uris = [track['track']['uri'] for track in liked_songs['items']]

    # Shuffle the song URIs
    random.shuffle(song_uris)

    # Create a new playlist
    playlist_name = 'Shuffled Liked Songs'  # Change the playlist name as desired
    playlist_description = 'Playlist of shuffled liked songs'

    user_id = sp.current_user()['id']
    playlist = sp.user_playlist_create(
        user_id, playlist_name, public=False, description=playlist_description)

    # Add the shuffled songs to the playlist
    sp.playlist_add_items(playlist['id'], song_uris)

    print(f'Playlist "{playlist_name}" created successfully with {len(song_uris)} songs!')

except spotipy.SpotifyException as e:
    print("An error occurred with the Spotify API:")
    print(e)

except requests.exceptions.RequestException as e:
    print("A network connection error occurred:")
    print(e)
