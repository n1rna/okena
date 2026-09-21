/**
 * SpaceSelector.tsx — one dot per space, at the top of the project drawer.
 *
 * A space is a separate set of projects, agents, tasks and roots inside one
 * profile (QBL-430). Switching changes what the drawer below lists.
 *
 * The daemon owns which space is showing — one per profile — so a tap asks and
 * the next poll brings the answer back. That is also how a switch made on the
 * desktop reaches the phone. Spaces are added, renamed and deleted on the
 * desktop; this client shows them and moves between them.
 */

import React from 'react';
import { View, Text, Pressable, StyleSheet } from 'react-native';

import { useWorkspaceStore } from '../state';
import { OkenaColors } from '../theme';

export function SpaceSelector() {
  const spaces = useWorkspaceStore((s) => s.spaces);
  const activeSpace = useWorkspaceStore((s) => s.activeSpace);
  const activateSpace = useWorkspaceStore((s) => s.activateSpace);

  // One space is no choice, and a daemon from before spaces sends none.
  if (spaces.length < 2) return null;

  const activeName = spaces.find((s) => s.id === activeSpace)?.name ?? '';

  return (
    <View style={styles.row}>
      {spaces.map((space) => {
        const isActive = space.id === activeSpace;
        return (
          <Pressable
            key={space.id}
            accessibilityRole="button"
            accessibilityLabel={space.name}
            accessibilityState={{ selected: isActive }}
            hitSlop={8}
            onPress={() => activateSpace(space.id)}
            style={styles.dotHit}
          >
            <View
              style={[
                styles.dot,
                isActive && styles.dotActive,
                // An agent waiting on you colours the dot even from a space
                // you have left — that is what it is there to say.
                space.agentWaiting && styles.dotWaiting,
              ]}
            />
          </Pressable>
        );
      })}
      <View style={styles.spacer} />
      <Text style={styles.activeName} numberOfLines={1}>
        {activeName}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 16,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  dotHit: { alignItems: 'center', justifyContent: 'center', width: 16, height: 16 },
  dot: {
    width: 7,
    height: 7,
    borderRadius: 4,
    backgroundColor: OkenaColors.textTertiary,
  },
  dotActive: {
    width: 9,
    height: 9,
    borderRadius: 5,
    backgroundColor: OkenaColors.textPrimary,
    borderWidth: 1,
    borderColor: OkenaColors.accent,
  },
  dotWaiting: { backgroundColor: OkenaColors.warning },
  spacer: { flex: 1 },
  activeName: { color: OkenaColors.textSecondary, fontSize: 11 },
});
